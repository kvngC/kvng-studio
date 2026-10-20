//! Shared memory and named semaphores used to exchange audio with a helper.

use std::ffi::CString;
use std::io;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

pub const MAX_BLOCK: usize = 512;
pub const CHANNELS: usize = 2;
pub const MAX_MIDI: usize = 1024;
/// Parameter changes carried with one block.
pub const MAX_PARAMS: usize = 64;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct RawMidi {
    pub offset: u32,
    pub bytes: [u8; 3],
    pub len: u8,
}

/// The block both processes map. The client fills the inputs, posts `request`,
/// the helper processes in place and posts `done`.
/// Identifies the block's memory layout; a helper built with a different layout must
/// not touch it (it would read audio from the wrong place).
pub fn layout_id() -> u32 {
    const VERSION: u32 = 2;
    (std::mem::size_of::<Block>() as u32) ^ (VERSION << 24)
}

/// Exit code of a helper that refuses a block with a different layout.
pub const EXIT_OUTDATED: i32 = 3;

#[repr(C)]
pub struct Block {
    /// [`layout_id`] of the app that created the block.
    pub layout: AtomicU32,
    pub num_samples: AtomicU32,
    pub num_midi: AtomicU32,
    /// Set by the client to make the helper's audio thread exit.
    pub quit: AtomicU32,
    /// Updated by the helper after each block.
    pub latency: AtomicU32,
    pub midi: [RawMidi; MAX_MIDI],
    pub num_params: AtomicU32,
    /// (parameter index, normalised value) applied before processing the block.
    pub params: [(u32, f32); MAX_PARAMS],
    pub audio: [[f32; MAX_BLOCK]; CHANNELS],
}

pub struct SharedBlock {
    ptr: *mut Block,
    name: CString,
    owner: bool,
}

// The mapping is process-shared memory; access is synchronised by the semaphores
// (one side touches the audio/MIDI fields at a time), other fields are atomics.
unsafe impl Send for SharedBlock {}
unsafe impl Sync for SharedBlock {}

fn os_err(what: &str) -> io::Error {
    let e = io::Error::last_os_error();
    io::Error::new(e.kind(), format!("{what}: {e}"))
}

impl SharedBlock {
    pub fn create(name: &str) -> io::Result<Self> {
        Self::open_impl(name, true)
    }

    pub fn open(name: &str) -> io::Result<Self> {
        Self::open_impl(name, false)
    }

    fn open_impl(name: &str, create: bool) -> io::Result<Self> {
        let cname = CString::new(name).unwrap();
        let size = std::mem::size_of::<Block>();
        unsafe {
            let flags = if create { libc::O_CREAT | libc::O_EXCL | libc::O_RDWR } else { libc::O_RDWR };
            let fd = libc::shm_open(cname.as_ptr(), flags, 0o600 as libc::c_uint);
            if fd < 0 {
                return Err(os_err("shm_open"));
            }
            if create && libc::ftruncate(fd, size as libc::off_t) != 0 {
                let e = os_err("ftruncate");
                libc::close(fd);
                libc::shm_unlink(cname.as_ptr());
                return Err(e);
            }
            let ptr = libc::mmap(std::ptr::null_mut(), size, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_SHARED, fd, 0);
            libc::close(fd);
            if ptr == libc::MAP_FAILED {
                return Err(os_err("mmap"));
            }
            Ok(Self { ptr: ptr as *mut Block, name: cname, owner: create })
        }
    }

    /// Removes the name so no other process can open it; the mapping stays valid.
    pub fn unlink(&mut self) {
        if self.owner {
            unsafe { libc::shm_unlink(self.name.as_ptr()) };
            self.owner = false;
        }
    }

    #[allow(clippy::mut_from_ref)]
    pub fn get(&self) -> &mut Block {
        unsafe { &mut *self.ptr }
    }
}

impl Drop for SharedBlock {
    fn drop(&mut self) {
        self.unlink();
        unsafe { libc::munmap(self.ptr as *mut libc::c_void, std::mem::size_of::<Block>()) };
    }
}

/// A POSIX named semaphore (macOS has no unnamed process-shared semaphores).
pub struct Semaphore {
    sem: *mut libc::sem_t,
    name: CString,
    owner: bool,
}

unsafe impl Send for Semaphore {}
unsafe impl Sync for Semaphore {}

impl Semaphore {
    pub fn create(name: &str) -> io::Result<Self> {
        let cname = CString::new(name).unwrap();
        let sem = unsafe { libc::sem_open(cname.as_ptr(), libc::O_CREAT | libc::O_EXCL, 0o600 as libc::c_uint, 0 as libc::c_uint) };
        if sem == libc::SEM_FAILED {
            return Err(os_err("sem_open"));
        }
        Ok(Self { sem, name: cname, owner: true })
    }

    pub fn open(name: &str) -> io::Result<Self> {
        let cname = CString::new(name).unwrap();
        let sem = unsafe { libc::sem_open(cname.as_ptr(), 0) };
        if sem == libc::SEM_FAILED {
            return Err(os_err("sem_open"));
        }
        Ok(Self { sem, name: cname, owner: false })
    }

    pub fn unlink(&mut self) {
        if self.owner {
            unsafe { libc::sem_unlink(self.name.as_ptr()) };
            self.owner = false;
        }
    }

    pub fn post(&self) {
        unsafe { libc::sem_post(self.sem) };
    }

    /// Blocks until posted.
    pub fn wait(&self) {
        while unsafe { libc::sem_wait(self.sem) } != 0 {
            if io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
                return;
            }
        }
    }

    /// Waits until posted or `timeout` passes (macOS lacks `sem_timedwait`).
    /// Spins briefly first, since helpers usually answer within microseconds.
    pub fn wait_timeout(&self, timeout: Duration) -> bool {
        let start = Instant::now();
        let mut spins = 0u32;
        loop {
            if unsafe { libc::sem_trywait(self.sem) } == 0 {
                return true;
            }
            if start.elapsed() >= timeout {
                return false;
            }
            spins += 1;
            if spins < 2000 {
                std::hint::spin_loop();
            } else {
                std::thread::yield_now();
            }
        }
    }
}

impl Drop for Semaphore {
    fn drop(&mut self) {
        self.unlink();
        unsafe { libc::sem_close(self.sem) };
    }
}

pub fn store_latency(block: &Block, latency: usize) {
    block.latency.store(latency as u32, Ordering::Relaxed);
}
