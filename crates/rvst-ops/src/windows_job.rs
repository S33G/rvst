//! Keep every owned subprocess in a kill-on-close Windows Job Object.
use std::{io, mem, os::windows::io::AsRawHandle, process::Child, ptr};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE},
    System::{
        Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
        },
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject, TerminateJobObject,
        },
        Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME},
    },
};
struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: this type owns a non-null, valid kernel handle.
        unsafe {
            CloseHandle(self.0);
        }
    }
}
pub(super) struct Job(Handle);
impl Job {
    pub(super) fn new() -> io::Result<Self> {
        // SAFETY: null security/name arguments create an unnamed, private job.
        let raw = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        let job = Self(Handle(raw));
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: pointer and size identify the documented information struct.
        if unsafe {
            SetInformationJobObject(
                raw,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                mem::size_of_val(&limits) as u32,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(job)
    }
    pub(super) fn attach_and_resume(&self, child: &Child) -> io::Result<()> {
        // The child was created suspended. Assign ownership before any of its
        // code executes, preventing even short-lived launchers escaping the job.
        // SAFETY: both handles remain valid throughout this call.
        if unsafe { AssignProcessToJobObject(self.0.0, child.as_raw_handle().cast()) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // std::process::Child exposes the process handle, but not its primary
        // thread. A suspended fresh process has exactly one thread; discover it
        // by its verified owner PID, then resume that thread only.
        // SAFETY: snapshot has no pointer arguments.
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let snapshot = Handle(snapshot);
        let mut entry = THREADENTRY32 {
            dwSize: mem::size_of::<THREADENTRY32>() as u32,
            ..Default::default()
        };
        // SAFETY: entry has its required size and snapshot remains live.
        let mut found = unsafe { Thread32First(snapshot.0, &mut entry) };
        while found != 0 {
            if entry.th32OwnerProcessID == child.id() {
                // SAFETY: thread ID came from the snapshot for our suspended child.
                let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) };
                if thread.is_null() {
                    return Err(io::Error::last_os_error());
                }
                let thread = Handle(thread);
                // SAFETY: owned handle has THREAD_SUSPEND_RESUME access.
                if unsafe { ResumeThread(thread.0) } == u32::MAX {
                    return Err(io::Error::last_os_error());
                }
                return Ok(());
            }
            // SAFETY: same valid snapshot and initialized entry.
            found = unsafe { Thread32Next(snapshot.0, &mut entry) };
        }
        Err(io::Error::other(
            "could not find the suspended subprocess thread",
        ))
    }
    pub(super) fn terminate(&self) -> io::Result<()> {
        // SAFETY: the private job handle remains live and owns only this task.
        if unsafe { TerminateJobObject(self.0.0, 130) } == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}
