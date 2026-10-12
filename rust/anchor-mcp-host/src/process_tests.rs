use super::*;
use std::{
    path::Path,
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

fn process_exists(pid: &str) -> bool {
    Path::new("/proc").join(pid.trim()).exists()
}

#[tokio::test]
async fn startup_timeout_reaps_the_actual_child() {
    let root = tempfile::tempdir().unwrap();
    let pidfile = root.path().join("pid");
    let mut command = tokio::process::Command::new("python3");
    command
        .args([
            "-c",
            "import os,time,sys; open(sys.argv[1],'w').write(str(os.getpid())); time.sleep(60)",
        ])
        .arg(&pidfile);
    let result = McpHost::connect_sandboxed_stdio_controlled(
        "startup-timeout".into(),
        command,
        Arc::new(AtomicBool::new(false)),
        Duration::from_millis(500),
    )
    .await;
    assert!(matches!(result,Err(McpHostError::Connect(reason)) if reason.contains("timed out")));
    let pid = std::fs::read_to_string(pidfile).unwrap();
    assert!(
        !process_exists(&pid),
        "startup timeout returned before child {pid} exited"
    );
}

#[tokio::test]
async fn shutdown_after_a_call_timeout_reaps_the_actual_child() {
    let root = tempfile::tempdir().unwrap();
    let pidfile = root.path().join("pid");
    let script = r#"import json,os,sys,time
open(sys.argv[1],'w').write(str(os.getpid()))
for line in sys.stdin:
    m=json.loads(line)
    if 'id' not in m: continue
    if m['method']=='initialize': r={'protocolVersion':'2025-03-26','capabilities':{'tools':{}},'serverInfo':{'name':'hang','version':'1'}}
    elif m['method']=='tools/list': r={'tools':[{'name':'hang','inputSchema':{'type':'object'}}]}
    elif m['method']=='tools/call':
        while True: time.sleep(.1)
    else: r={}
    print(json.dumps({'jsonrpc':'2.0','id':m['id'],'result':r}),flush=True)
"#;
    let mut command = tokio::process::Command::new("python3");
    command.args(["-c", script]).arg(&pidfile);
    let host = McpHost::connect_sandboxed_stdio_controlled(
        "call-timeout".into(),
        command,
        Arc::new(AtomicBool::new(false)),
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    let pid = std::fs::read_to_string(pidfile).unwrap();
    assert!(process_exists(&pid));
    assert!(
        tokio::time::timeout(
            Duration::from_millis(100),
            host.call("hang", serde_json::json!({}))
        )
        .await
        .is_err()
    );
    host.shutdown().await.unwrap();
    assert!(
        !process_exists(&pid),
        "shutdown returned before child {pid} exited"
    );
}
