use super::*;
use tokio::sync::mpsc;

fn shell(script: &str) -> Spec {
    let dir = std::env::temp_dir();
    if cfg!(windows) {
        Spec { program: "cmd".into(), args: vec!["/C".into(), script.into()], dir, env: Vec::new() }
    } else {
        Spec { program: "sh".into(), args: vec!["-c".into(), script.into()], dir, env: Vec::new() }
    }
}

fn tag() -> Tag {
    Tag::Build { key: "x".into(), job: 1 }
}

#[tokio::test]
async fn output_then_the_exit_code() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let _child = Child::spawn(&shell("echo one&& echo two&& exit 3"), tag(), tx, None).unwrap();
    let mut lines = Vec::new();
    loop {
        match rx.recv().await.unwrap() {
            Event::Output { line, .. } => lines.push(line.trim().to_string()),
            Event::Exited { code, .. } => {
                assert_eq!(code, Some(3));
                break;
            }
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(lines, ["one", "two"]);
}

#[tokio::test]
async fn kill_takes_the_grandchildren_too() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    // A shell that starts a long sleep and waits: killing the tree must end both.
    let script = if cfg!(windows) { "ping -n 60 127.0.0.1 > nul" } else { "sleep 60 & wait" };
    let child = Child::spawn(&shell(script), tag(), tx, None).unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    child.kill();
    let exited = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(Event::Exited { .. }) = rx.recv().await {
                return;
            }
        }
    })
    .await;
    assert!(exited.is_ok(), "the tree ended");
}

/// Waits for a line tagged `wanted`.
#[cfg(windows)]
async fn line_from(rx: &mut mpsc::UnboundedReceiver<Event>, wanted: &Tag) -> String {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match rx.recv().await {
                Some(Event::Output { tag, line }) if tag == *wanted => return line,
                Some(_) => {}
                None => panic!("the events ended"),
            }
        }
    })
    .await
    .expect("output arrives")
}

/// Whether a process with this id is running.
#[cfg(windows)]
fn alive(pid: u32) -> bool {
    let filter = format!("PID eq {pid}");
    let out = std::process::Command::new("tasklist").args(["/FI", &filter, "/NH"]).output().unwrap();
    String::from_utf8_lossy(&out.stdout).contains(&pid.to_string())
}

/// Handovers in one process: released, the tree keeps running; taken over, its output and exit arrive under the
/// new tag - and a taken-over tree can be handed over again. Dropping the last owner kills it.
#[cfg(windows)]
#[tokio::test]
async fn a_tree_can_be_handed_over_again_and_again() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let script = "for /L %i in (1,1,600) do @(echo tick %i & ping -n 2 127.0.0.1 > nul)";
    let mut child = Child::spawn(&shell(script), tag(), tx.clone(), None).unwrap();
    let pid = child.pid;
    line_from(&mut rx, &tag()).await;
    for generation in [7, 8] {
        let released = child.release().unwrap();
        let next = Tag::App { key: "y".into(), generation };
        // SAFETY: in a real handover these copies are the next lazyrunner's; here adopt takes them over, so
        // `released` mustn't close them too.
        child = unsafe { Child::adopt(released.handles, next.clone(), tx.clone()) }.unwrap();
        std::mem::forget(released);
        assert_eq!(child.pid, pid, "the same process");
        assert!(line_from(&mut rx, &next).await.contains("tick"), "output keeps arriving");
    }
    drop(child);
    let exited = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(Event::Exited { tag: Tag::App { generation: 8, .. }, .. }) = rx.recv().await {
                return;
            }
        }
    })
    .await;
    assert!(exited.is_ok(), "dropping the last owner killed it");
}

/// Handles that don't match what the handover says are refused and left alone; ending a tree that isn't taken
/// over (`discard`) kills it.
#[cfg(windows)]
#[tokio::test]
async fn wrong_handles_are_refused_and_a_discarded_tree_ends() {
    let (tx, _rx) = mpsc::unbounded_channel();
    let child = Child::spawn(&shell("ping -n 30 127.0.0.1 > nul"), tag(), tx.clone(), None).unwrap();
    let pid = child.pid;
    let released = child.release().unwrap();
    let wrong = Handles { pid: pid + 1, ..released.handles };
    // SAFETY: the check fails (another process id) before anything is taken over.
    assert!(unsafe { Child::adopt(wrong, tag(), tx) }.is_err(), "a process with another id is refused");
    assert!(alive(pid), "refusing left the tree running");
    // SAFETY: discard closes the copies `released` holds, so `released` mustn't close them too.
    unsafe { discard(&released.handles, true) };
    std::mem::forget(released);
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while alive(pid) && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(!alive(pid), "discarding killed the tree");
}

#[cfg(unix)]
#[tokio::test]
async fn no_grandchild_survives_the_kill() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let child = Child::spawn(&shell("sleep 60 & echo $!; wait"), tag(), tx, None).unwrap();
    let pid = loop {
        if let Some(Event::Output { line, .. }) = rx.recv().await {
            break line.trim().parse::<i32>().unwrap();
        }
    };
    child.kill();
    tokio::time::sleep(Duration::from_millis(500)).await;
    // SAFETY: signal 0 only asks whether the process exists.
    let alive = unsafe { libc::kill(pid, 0) == 0 };
    assert!(!alive, "the backgrounded sleep {pid} is gone too");
}
