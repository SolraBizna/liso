//! Contains all the logic for the stderr capture.
//!
//! We don't use `nix` for this stuff because we will also attempt it on
//! Windows using nearly-identical code, and it wouldn't make sense to have
//! almost-totally-parallel nix-based safe code and non-nix-based unsafe code.

use std::thread::JoinHandle;

use crossterm::tty::IsTty;
use libc::c_int;
use parking_lot::Mutex;

const STDERR_FD: c_int = 2;
static STDERR_CAPTURE_THREAD: Mutex<Option<JoinHandle<()>>> = Mutex::new(None);

fn pipe() -> Result<(c_int, c_int), c_int> {
    #[cfg(any(target_family = "windows", target_family = "unix"))]
    loop {
        let mut fds = [0; 2];
        #[cfg(target_family = "windows")]
        let result =
            unsafe { libc::pipe((&mut fds).as_mut_ptr(), 128, libc::O_TEXT) };
        #[cfg(target_family = "unix")]
        let result = unsafe { libc::pipe((&mut fds).as_mut_ptr()) };
        if result == 0 {
            return Ok((fds[0], fds[1]));
        } else {
            let errno = errno::errno().0;
            if errno == libc::EINTR {
                continue;
            } else {
                return Err(errno);
            }
        }
    }
    #[allow(unreachable_code)]
    return Err(libc::ENOSYS);
}

fn dup2(src: c_int, dst: c_int) -> Result<(), c_int> {
    #[cfg(any(target_family = "windows", target_family = "unix"))]
    loop {
        errno::set_errno(errno::Errno(0));
        let result = unsafe { libc::dup2(src, dst) };
        if result >= 0 {
            if result != dst {
                let errno = errno::errno().0;
                let errno = if errno == 0 { libc::ENOSYS } else { errno };
                unsafe {
                    libc::close(result);
                }
                return Err(errno);
            }
            return Ok(());
        } else {
            let errno = errno::errno().0;
            if errno == libc::EINTR {
                continue;
            } else {
                return Err(errno);
            }
        }
    }
    #[allow(unreachable_code)]
    return Err(libc::ENOSYS);
}

fn dup(src: c_int) -> Result<c_int, c_int> {
    #[cfg(any(target_family = "windows", target_family = "unix"))]
    loop {
        errno::set_errno(errno::Errno(0));
        let result = unsafe { libc::dup(src) };
        if result >= 0 {
            return Ok(result);
        } else {
            let errno = errno::errno().0;
            if errno == libc::EINTR {
                continue;
            } else {
                return Err(errno);
            }
        }
    }
    #[allow(unreachable_code)]
    return Err(libc::ENOSYS);
}

fn close(_fd: c_int) {
    #[cfg(any(target_family = "windows", target_family = "unix"))]
    unsafe {
        libc::close(_fd);
    }
}

fn read(fd: c_int, buf: &mut [u8]) -> Result<usize, c_int> {
    #[cfg(any(target_family = "windows", target_family = "unix"))]
    loop {
        let result = unsafe {
            libc::read(
                fd,
                std::mem::transmute(buf.as_mut_ptr()),
                buf.len() as libc::size_t,
            )
        };
        if result >= 0 {
            return Ok(result as usize);
        } else {
            let errno = errno::errno().0;
            if errno == libc::EINTR {
                continue;
            } else {
                return Err(errno);
            }
        }
    }
    #[allow(unreachable_code)]
    return Err(libc::ENOSYS);
}

fn write_all(fd: c_int, mut buf: &[u8]) -> Result<(), c_int> {
    #[cfg(any(target_family = "windows", target_family = "unix"))]
    while !buf.is_empty() {
        let result = unsafe {
            libc::write(
                fd,
                std::mem::transmute(buf.as_ptr()),
                buf.len() as libc::size_t,
            )
        };
        if result >= 0 {
            buf = &buf[result as usize..];
        } else {
            let errno = errno::errno().0;
            if errno == libc::EINTR {
                continue;
            } else {
                return Err(errno);
            }
        }
    }
    if buf.is_empty() {
        return Ok(());
    } else {
        #[allow(unreachable_code)]
        return Err(libc::ENOSYS);
    }
}

pub(crate) fn attempt_stderr_capture(output: crate::Output) {
    // wait until previous stderr capture is over, just in case `InputOutput`s
    // are created and destroyed quickly
    let mut lock;
    loop {
        wait_until_not_captured();
        lock = STDERR_CAPTURE_THREAD.lock();
        match lock.as_ref() {
            None => break,
            Some(_) => continue,
        }
    }
    if !std::io::stderr().is_tty() {
        return;
    }
    let (r, w) = match pipe() {
        Ok(x) => x,
        Err(x) => {
            let _ = output.tx.send(crate::Request::StderrLine(format!(
                "pipe() returned error {:?} when attempting to capture stderr.",
                x
            )));
            return;
        }
    };
    let real_stderr = match dup(STDERR_FD) {
        Ok(x) => x,
        Err(x) => {
            let _ = output.tx.send(crate::Request::StderrLine(format!(
                "dup(STDERR_FD) returned error {:?} when attempting to capture stderr.",
                x
            )));
            return;
        }
    };
    if let Err(x) = dup2(w, STDERR_FD) {
        close(r);
        close(w);
        let _ = output.tx.send(crate::Request::StderrLine(format!(
            "dup2() returned error {:?} when attempting to capture stderr.",
            x
        )));
        return;
    }
    close(w); // it is now staying alive as STDERR_FD
    *lock = Some(std::thread::spawn(move || {
        let mut buf = vec![0u8; 128];
        let mut buf_pos = 0;
        'outer: loop {
            if buf_pos == buf.len() {
                buf.resize(buf.len() + 128, 0u8);
            }
            match read(r, &mut buf[buf_pos..]) {
                Ok(0) => {
                    // stderr ended?!
                    if buf_pos > 0 {
                        if let Err(_) =
                            output.tx.send(crate::Request::StderrLine(
                                String::from_utf8_lossy(&buf[..buf_pos])
                                    .to_string(),
                            ))
                        {
                            let _ = write_all(real_stderr, &buf[..buf_pos]);
                        }
                    }
                    buf_pos = 0;
                    break;
                }
                Ok(x) => {
                    let mut last_newline_pos = None;
                    let end_pos = buf_pos + x;
                    while let Some(p) = buf[buf_pos..]
                        .iter()
                        .position(|x| *x == b'\n')
                        .map(|x| x + buf_pos)
                    {
                        let start_pos =
                            last_newline_pos.map(|x| x + 1).unwrap_or(0);
                        if let Err(_) =
                            output.tx.send(crate::Request::StderrLine(
                                String::from_utf8_lossy(&buf[start_pos..p])
                                    .to_string(),
                            ))
                        {
                            // can't do anything sensible with an error here
                            let _ = write_all(
                                real_stderr,
                                &buf[start_pos..end_pos],
                            );
                            buf_pos = 0;
                            break 'outer;
                        }
                        last_newline_pos = Some(p);
                        buf_pos = p + 1;
                    }
                    buf_pos = end_pos;
                    if let Some(p) = last_newline_pos {
                        buf.copy_within(p + 1.., 0);
                        buf_pos -= p + 1;
                    }
                }
                Err(x) => {
                    if buf_pos > 0 {
                        if let Err(_) =
                            output.tx.send(crate::Request::StderrLine(
                                String::from_utf8_lossy(&buf[..buf_pos])
                                    .to_string(),
                            ))
                        {
                            let _ = write_all(real_stderr, &buf[..buf_pos]);
                        }
                    }
                    let _ =
                        output.tx.send(crate::Request::StderrLine(format!(
                        "read() returned error {:?} when reading from stderr.",
                        x
                    )));
                    buf_pos = 0;
                    break;
                }
            }
        }
        assert_eq!(
            buf_pos, 0,
            "INTERNAL LISO ERROR: buf contents not fully handled when liso closed down!"
        );
        // Small possibility that some bytes will be mixed up if a lot of
        // stderr output is happening at once. Oh well. That's an unavoidable
        // cost of your program bypassing the "so" part of "liso".
        //
        // There's also a small possibility that one or more StderrLines we
        // sent "successfully" were lost. Oh well.
        dup2(real_stderr, STDERR_FD)
            .expect("Unable to reduplicate stderr back into place!");
        close(real_stderr);
        // Any remaining output waiting in the pipe, process.
        while let Ok(amount) = read(r, &mut buf[..]) {
            if amount == 0 {
                break;
            }
            let _ = write_all(STDERR_FD, &buf[..amount]);
        }
        close(r);
    }));
}

pub(crate) fn wait_until_not_captured() {
    let mut lock = STDERR_CAPTURE_THREAD.lock();
    if let Some(x) = lock.take() {
        close(STDERR_FD); // :(
        let _ = x.join();
    }
    // Do not drop the lock until here! Nobody else should be allowed to
    // think they can join before us!
    drop(lock);
}
