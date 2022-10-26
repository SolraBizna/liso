use std::{
    env,
    io::{self, Read, BufRead, BufReader, Write},
    sync::mpsc,
    process::{self, ExitStatus},
    time::Duration,
};

use liso::{Output, OutputOnly, liso, liso_add};

const HELP: &str = include_str!("../README.md");

enum Custom {
    JobOver(usize, io::Result<ExitStatus>),
}

struct Shell {
    cwd_name: String,
    jobs: Vec<Option<Job>>,
    output: OutputOnly,
    target_job: Option<usize>,
}

struct Job {
    job_name: String,
    job_line: String,
    stdin_tx: mpsc::Sender<Option<String>>,
    pid: u32,
    kill_count: u32,
}

fn find_cwd_name() -> String {
    match env::current_dir() {
        Ok(x) => {
            if let Some(name) = x.components().rev().next() {
                return name.as_os_str().to_string_lossy().to_string()
            }
        },
        _ => ()
    }
    "???".to_string()
}

impl Shell {
    fn new(output: OutputOnly) -> Shell {
        let mut ret = Shell {
            cwd_name: find_cwd_name(),
            jobs: vec![],
            output,
            target_job: None,
        };
        ret.update_prompt();
        ret.update_status();
        ret
    }
    fn err(&self, err: &str) {
        self.output.wrapln(liso!(fg=red, bold, "< ", err));
    }
    fn warn(&self, err: &str) {
        self.output.wrapln(liso!(fg=yellow, bold, "< ", err));
    }
    fn ok(&self, err: &str) {
        self.output.wrapln(liso!(fg=green, bold, "< ", err));
    }
    /// Handles a command line of shell input. Returns Some(status) if the
    /// program should exit.
    fn input(&mut self, input: String) -> Option<i32> {
        if let Some(id) = self.target_job {
            if let Some(job) = self.jobs.get(id).and_then(|x| x.as_ref()) {
                self.output.echoln(liso!(dim, &job.job_name, fg=blue, bold, format!("[{}]> ", id), fg=none, &input));
                let _ = job.stdin_tx.send(Some(input));
                return None;
            }
            else {
                self.warn("No job with that ID? (bug!)");
                self.next_target();
                return None
            }
        }
        // If we got here, it's shell input.
        self.output.echoln(liso!(fg=green, bold, &self.cwd_name, "> ", fg=none, &input));
        let our_parse = shellish_parse::parse(&input, true);
        let job_name;
        match our_parse {
            Ok(mut elements) => {
                if elements.is_empty() { return None }
                job_name = elements.remove(0);
                match job_name.as_str() {
                    "exit" | "quit" | "bye" => return self.builtin_exit(elements),
                    "cd" => return self.builtin_cd(elements),
                    "help" => return self.builtin_help(elements),
                    "export" => return self.builtin_setenv(elements),
                    "unsetenv" => return self.builtin_unsetenv(elements),
                    "setenv" => return self.builtin_setenv(elements),
                    "fg" => return self.builtin_fg(elements),
                    "bg" => return self.builtin_bg(elements),
                    "jobs" => return self.builtin_jobs(elements),
                    _ => (),
                }
                // if we reached this point, it was not a builtin command
                if cfg!(not(unix)) || env::var("NOSH").map(|x| !x.is_empty() && x != "0").unwrap_or(false) {
                    Job::spawn_nosh(self, job_name.clone(), job_name, input, elements);
                    self.update_status();
                    return None;
                }
            },
            Err(x) => {
                self.err(&format!("Error parsing command line: {}", x));
                return None;
            },
        }
        // if we reached this point, we get to be UNIX :)
        Job::spawn_sh(self, job_name, input);
        self.update_status();
        return None;
    }
    fn job_over(&mut self, id: usize, how: io::Result<ExitStatus>) {
        assert!(id < self.jobs.len());
        match how {
            Ok(x) => {
                if x.success() {
                    self.ok(&format!("Job [{}] completed.", id));
                }
                else {
                    self.err(&format!("Job [{}] exited with {}", id, x));
                }
            },
            Err(x) => {
                self.err(&format!("Error waiting for job [{}]: {}", id, x));
            },
        }
        self.jobs[id] = None;
        self.update_status();
        if self.target_job == Some(id) {
            self.next_target();
        }
    }
    fn next_target(&mut self) {
        if self.jobs.is_empty() {
            self.target_job = None;
            return;
        }
        loop {
            self.target_job = match self.target_job {
                None => Some(0),
                Some(x) if x < self.jobs.len()-1 => Some(x+1),
                Some(_) => None,
            };
            match self.target_job {
                None => break,
                Some(x) if self.jobs[x].is_some() => break,
                _ => (),
            }
        }
    }
    fn update_prompt(&self) {
        match self.target_job {
            None => self.output.prompt(liso!(fg=green, bold, &self.cwd_name, "> ", reset), true, false),
            Some(x) => self.output.prompt(liso!(dim, self.jobs.get(x).and_then(|x|x.as_ref()).map(|x| x.job_name.as_str()).unwrap_or(""), fg=blue, bold, format!("[{}]> ", x), reset), true, false),
        }
    }
    fn update_status(&mut self) {
        while self.jobs.last().map(|x| x.is_none()).unwrap_or(false) {
            self.jobs.remove(self.jobs.len()-1);
        }
        if self.jobs.is_empty() {
            self.output.status(Some(liso!(inverse, fg=green, " No jobs running.")));
            return;
        }
        let mut line = liso!(inverse, fg=green, +bold, " Running:", -bold);
        for (i, job) in self.jobs.iter().enumerate() {
            if job.is_some() {
                liso_add!(line, format!(" {}", i));
            }
        }
        self.output.status(Some(line));
    }
    fn finish(&mut self) -> Option<i32> {
        if let Some(id) = self.target_job {
            if let Some(job) = self.jobs.get(id).and_then(|x| x.as_ref()) {
                self.output.echoln(liso!(dim, &job.job_name, fg=blue, bold, format!("[{}]> ", id), fg=none, inverse, "^D", -inverse));
                let _ = job.stdin_tx.send(None);
                return None
            }
            else {
                self.warn("No job with that ID? (bug!)");
                self.next_target();
                return None
            }
        }
        // If we got here, we're at the shell prompt.
        self.output.echoln(liso!(fg=green, bold, &self.cwd_name, "> ", fg=none, inverse, "^D", -inverse));
        self.builtin_exit(vec![])
    }
    fn info(&mut self) {
        if let Some(id) = self.target_job {
            if let Some(job) = self.jobs.get(id).and_then(|x| x.as_ref()) {
                self.output.echoln(liso!(dim, &job.job_name, fg=blue, bold, format!("[{}]> ", id), fg=none, inverse, "^T", -inverse));
                job.getinfo(id, &self.output);
                return
            }
            else {
                self.warn("No job with that ID? (bug!)");
                self.next_target();
                return
            }
        }
        // If we got here, we're at the shell prompt.
        self.output.echoln(liso!(fg=green, bold, &self.cwd_name, "> ", fg=none, inverse, "^T", -inverse));
        self.builtin_jobs(vec![]);
    }
    fn quit(&mut self) -> Option<i32> {
        if let Some(id) = self.target_job {
            if let Some(job) = self.jobs.get_mut(id).and_then(|x| x.as_mut()) {
                self.output.echoln(liso!(dim, &job.job_name, fg=blue, bold, format!("[{}]> ", id), fg=none, inverse, "^C", -inverse));
                job.kill();
                return None
            }
            else {
                self.warn("No job with that ID? (bug!)");
                self.next_target();
                return None
            }
        }
        // If we got here, we're at the shell prompt.
        self.output.echoln(liso!(fg=green, bold, &self.cwd_name, "> ", fg=none, inverse, "^C", -inverse));
        self.builtin_exit(vec![])
    }
    fn builtin_cd(&mut self, mut args: Vec<String>) -> Option<i32> {
        if args.is_empty() {
            let home = env::var("HOME").unwrap_or_else(|_| env::var("USERPROFILE").unwrap_or_else(|_| "/".to_string()));
            args.push(home);
        }
        for target in args.into_iter() {
            match env::set_current_dir(&target) {
                Ok(_) => {
                    self.cwd_name = find_cwd_name();   
                },
                Err(x) => {
                    self.err(&format!("{:?}: {}", target, x));
                    break;
                },
            }
        }
        self.update_prompt();
        return None
    }
    fn builtin_exit(&mut self, args: Vec<String>) -> Option<i32> {
        let status = match args.len() {
            0 => Some(0),
            1 => match args[0].parse::<i32>() {
                Ok(x) => Some(x),
                Err(_) => None,
            },
            _ => None,
        };
        let status = match status {
            Some(x) => x,
            None => {
                self.err("Usage: exit [status]");
                self.err("status should be an integer from -128 to 127");
                return None
            },
        };
        if !self.jobs.is_empty() {
            self.err("Cannot exit now. There are jobs running.");
            return None
        }
        return Some(status)
    }
    fn builtin_help(&mut self, args: Vec<String>) -> Option<i32> {
        if args.is_empty() {
            for line in HELP.lines() {
                self.output.wrapln(liso!(fg=green, bold, "> ", reset, line));
            }
        }
        else {
            self.warn("No detailed help, sorry.");
        }
        None
    }
    fn builtin_unsetenv(&mut self, args: Vec<String>) -> Option<i32> {
        for var in args.iter() {
            if env::var(var).is_ok() {
                env::remove_var(var);
            }
        }
        None
    }
    fn builtin_setenv(&mut self, args: Vec<String>) -> Option<i32> {
        for blah in args.iter() {
            match blah.find('=') {
                Some(idx) => {
                    let var = &blah[..idx];
                    let value = &blah[idx+1..];
                    env::set_var(var, value);
                },
                None => {
                    env::remove_var(blah);
                    continue
                },
            }
        }
        None
    }
    fn builtin_fg(&mut self, _args: Vec<String>) -> Option<i32> {
        self.err("There's no such thing as \"fg\" in this shell.");
        None
    }
    fn builtin_bg(&mut self, _args: Vec<String>) -> Option<i32> {
        self.err("There's no such thing as \"bg\" in this shell.");
        None
    }
    fn builtin_jobs(&mut self, args: Vec<String>) -> Option<i32> {
        if !args.is_empty() {
            self.err("Usage: jobs");
        }
        else if self.jobs.is_empty() {
            self.output.wrapln(liso!(fg=green, bold, "> ", reset, "No jobs are running."));
        }
        else {
            for (id, job) in self.jobs.iter().enumerate() {
                let job = match job {
                    Some(x) => x,
                    None => continue,
                };
                job.getinfo(id, &self.output);
            }
        }
        None
    }
}

fn pipe_reader<T: Read>(output: OutputOnly, reader: T, job_name: String, id: usize, error: bool) {
    let reader = BufReader::new(reader);
    for line in reader.lines() {
        match line {
            Ok(x) => {
                if error {
                    output.println(liso!(fg=red, dim, &job_name, bold, format!("[{}]< ", id), bold, x));
                }
                else {
                    output.println(liso!(dim, &job_name, bold, fg=blue, format!("[{}]< ", id), fg=none, plain, x));
                }
            },
            Err(_) => break,
        }
    }
}

impl Job {
    fn spawn_sh(shell: &mut Shell, job_name: String, job_line: String) {
        Job::spawn_nosh(shell, "/bin/sh".to_string(), job_name, job_line.clone(), vec!["-c".to_string(), job_line]);
    }
    fn spawn_nosh(shell: &mut Shell, process_name: String, job_name: String, job_line: String, args: Vec<String>) {
        use process::Stdio;
        let result = process::Command::new(process_name)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();
        match result {
            Ok(mut x) => {
                let pid = x.id();
                let target_id = shell.jobs.iter().enumerate().filter_map(|(i,x)| if x.is_none() { Some(i) } else { None }).next().unwrap_or(shell.jobs.len());
                shell.ok(&format!("Job [{}] started.", target_id));
                let output = shell.output.clone_output();
                let job_name_clone = job_name.clone();
                let stdout = x.stdout.take().unwrap();
                std::thread::spawn(move || pipe_reader(output, stdout, job_name_clone, target_id, false));
                let output = shell.output.clone_output();
                let job_name_clone = job_name.clone();
                let stderr = x.stderr.take().unwrap();
                std::thread::spawn(move || pipe_reader(output, stderr, job_name_clone, target_id, true));
                let mut stdin = x.stdin.take().unwrap();
                let (stdin_tx, stdin_rx) = mpsc::channel();
                std::thread::spawn(move || {
                    loop {
                        let mut line: String = match stdin_rx.recv() {
                            Ok(Some(x)) => x,
                            Ok(None) => break,
                            Err(_) => break,
                        };
                        line.push('\n');
                        if stdin.write_all(line.as_bytes()).is_err() {
                            break
                        }
                    }
                });
                let output = shell.output.clone_output();
                std::thread::spawn(move || {
                    let res = x.wait();
                    std::thread::sleep(Duration::from_millis(50));
                    output.send_custom(Custom::JobOver(target_id, res))
                });
                let new_job = Job {
                    job_name, job_line, stdin_tx, kill_count: 0, pid,
                };
                if target_id < shell.jobs.len() {
                    assert!(shell.jobs[target_id].is_none());
                    shell.jobs[target_id] = Some(new_job);
                }
                else {
                    assert_eq!(target_id, shell.jobs.len());
                    shell.jobs.push(Some(new_job));
                }
                shell.target_job = Some(target_id);
            },
            Err(x) => {
                shell.err(&format!("Error spawning command: {}", x));
            },
        }
    }
    fn getinfo(&self, id: usize, output: &Output) {
        output.wrapln(liso!(fg=green, bold, "> ", reset,
            format!("Job [{}]: {}", id, self.job_line)));
    }
    fn kill(&mut self) {
        #[cfg(unix)]
        {
            use nix::unistd::Pid;
            use nix::sys::signal::{kill, Signal};
            let pid = Pid::from_raw(self.pid as i32);
            if self.kill_count == 0 {
                let _ = kill(pid, Signal::SIGINT);
            }
            else if self.kill_count == 1 {
                let _ = kill(pid, Signal::SIGTERM);
            }
            else if self.kill_count == 2 {
                let _ = kill(pid, Signal::SIGKILL);
            }
            self.kill_count += 1;
        }
        #[cfg(not(unix))]
        {
            self.err("Can't kill jobs on this OS. Sorry.");
        }
    }
}

fn real_main() -> i32 {
    let mut io = liso::InputOutput::new();
    env::set_var("TERM", "");
    io.wrapln(liso!(bold, "Welcome to Lish! Type \"help\" for help."));
    let mut shell = Shell::new(io.clone_output());
    let mut old_target_job = Some(usize::MAX);
    loop {
        use liso::Response;
        if old_target_job != shell.target_job {
            old_target_job = shell.target_job;
            shell.update_prompt();
        }
        match io.read_blocking() {
            Response::Dead => return 1,
            Response::Discarded(x) => {
                io.echoln(liso!(fg=red, dim, "X ", x));
            },
            Response::Input(x) => {
                if let Some(status) = shell.input(x) { return status }
            },
            Response::Swap => shell.next_target(),
            Response::Finish => if let Some(status) = shell.finish() { return status },
            Response::Info => shell.info(),
            Response::Quit => if let Some(status) = shell.quit() { return status },
            Response::Custom(x) => {
                if let Ok(x) = x.downcast::<Custom>() {
                    match *x {
                        Custom::JobOver(target, how)
                            => shell.job_over(target, how),
                    }
                }
                else {
                    shell.warn("Received unknown Response::Custom");
                }
            },
            other => {
                io.notice(format!("unknown key {}",
                                       other.as_unknown() as char),
                               Duration::from_secs(1));
            },
        }
    }
}

fn main() {
    std::process::exit(real_main());
}