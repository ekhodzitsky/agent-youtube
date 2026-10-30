use serde_json::Value;
use std::{
    io::Write,
    process::{Command, Output, Stdio},
};

// The library's setup serializer test needs only validate; integration tests use both.
#[allow(dead_code)]
pub fn collect(output: &Output, documents: &mut Vec<Value>) {
    let start = output.stdout.iter().position(|byte| *byte == b'{').unwrap();
    let mut stream =
        serde_json::Deserializer::from_slice(&output.stdout[start..]).into_iter::<Value>();
    documents.push(stream.next().unwrap().unwrap());
    for line in output
        .stderr
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        documents.push(serde_json::from_slice(line).unwrap());
    }
}

pub fn validate(documents: &[Value]) {
    let mut child = Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tools/check_contract.py"
        ))
        .arg("--stdin")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Python and tools/check-requirements.txt are required for local checks");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(documents).unwrap())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "contract check: {} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
