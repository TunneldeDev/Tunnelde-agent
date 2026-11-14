//! A request process belongs to its SSH session; admitted IO must not.
use super::*;
use std::{mem::size_of, os::windows::io::AsRawHandle};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE},
    System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_BREAKAWAY_OK, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    },
};

struct Job(HANDLE);
// Cargo may itself use a restrictive job. Successful-request fixtures model
// the server's allowed policy explicitly; refusal is separately tested below.
pub(super) fn permitted_request_job(child: &std::process::Child) -> impl Drop {
    let job = Job::new(true);
    job.assign(child);
    job
}
impl Job {
    fn new(breakaway: bool) -> Self {
        // SAFETY: unnamed process-local object; no borrowed pointers retained.
        let job = Self(unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) });
        assert!(!job.0.is_null());
        // SAFETY: native POD structure accepts zero initialization.
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
            | if breakaway {
                JOB_OBJECT_LIMIT_BREAKAWAY_OK
            } else {
                0
            };
        // SAFETY: valid object, correctly sized initialized native structure.
        assert_ne!(
            unsafe {
                SetInformationJobObject(
                    job.0,
                    JobObjectExtendedLimitInformation,
                    (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
            },
            0
        );
        job
    }
    fn request(&self, fixture: &Fixture, request: &Value) -> Output {
        let mut child = fixture.command().spawn().unwrap();
        self.assign(&child);
        child
            .stdin
            .take()
            .unwrap()
            .write_all(&serde_json::to_vec(request).unwrap())
            .unwrap();
        child.wait_with_output().unwrap()
    }
    fn assign(&self, child: &std::process::Child) {
        // Waiting for stdin prevents launching a worker before assignment.
        // SAFETY: both handles are live; only our test child is assigned.
        assert_ne!(
            unsafe { AssignProcessToJobObject(self.0, child.as_raw_handle()) },
            0
        );
    }
}
impl Drop for Job {
    fn drop(&mut self) {
        // SAFETY: sole owner; this job never contains a user process.
        unsafe {
            CloseHandle(self.0);
        }
    }
}

#[test]
fn admitted_worker_survives_request_job_closure() {
    let fixture = Fixture::new();
    let content = "x".repeat(2 * 1024 * 1024);
    let request = fixture.submit("job.txt", &content, json!({"kind":"absent"}));
    let job = Job::new(true);
    let output = job.request(&fixture, &request);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        response["observation"]["operationId"],
        request["control"]["operationId"]
    );
    drop(job);
    let settled = fixture.settled(&request);
    assert_eq!(settled["observation"]["phase"], "succeeded", "{settled}");
    assert_eq!(
        fs::read_to_string(fixture.scope.join("job.txt")).unwrap(),
        content
    );
}

#[test]
fn restrictive_job_rejects_launch_without_fallback_or_losing_admission() {
    let fixture = Fixture::new();
    let request = fixture.submit("not-launched.txt", "retained", json!({"kind":"absent"}));
    let job = Job::new(false);
    let output = job.request(&fixture, &request);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        "Tunnelde helper: files.save.launchUnavailable\n"
    );
    drop(job);
    let status = fixture.status(&request);
    assert_eq!(status["observation"]["phase"], "prepared");
    assert_eq!(status["observation"]["live"], false);
    assert_eq!(status["observation"]["publication"], "none");
    assert!(!fixture.scope.join("not-launched.txt").exists());
}
