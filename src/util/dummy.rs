pub struct InterruptibleStdinThread;

impl InterruptibleStdinThread {
    pub fn new(
        _join_handle: std::thread::JoinHandle<()>,
    ) -> InterruptibleStdinThread {
        InterruptibleStdinThread
    }
    pub fn interrupt(&mut self) {
        // placebo!
    }
    pub fn placebo_check() {
        use std::sync::atomic::{AtomicBool, Ordering};
        static ONCE: AtomicBool = AtomicBool::new(false);
        loop {
            match ONCE.compare_exchange_weak(
                false,
                true,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => break,
                Err(false) => continue,
                Err(true) => {
                    panic!(
                        "Liso was instantiated more than once! (On this \
                         platform, Liso may only be instantiated once per \
                         run.)"
                    )
                }
            }
        }
    }
}
