//! Run heavy work in the background without costing the game frames.

/// Lower the current thread's CPU, disk and memory priority (Windows "background mode"), for
/// threads that analyse recordings. No-op elsewhere.
pub fn lower_current_thread() {
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        extern "system" {
            fn GetCurrentThread() -> isize;
            fn SetThreadPriority(thread: isize, priority: i32) -> i32;
        }
        const THREAD_MODE_BACKGROUND_BEGIN: i32 = 0x0001_0000;
        // SAFETY: GetCurrentThread returns a pseudo handle valid for the calling thread; the call
        // only changes this thread's scheduling.
        unsafe {
            SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN);
        }
    }
}
