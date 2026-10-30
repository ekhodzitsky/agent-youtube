//! A host application must dispatch the private worker before its own CLI.
fn main() -> agent_youtube::Result<()> {
    if agent_youtube::dispatch_web_worker(std::env::args_os().skip(1))? {
        return Ok(());
    }
    // Replace this with the host application's Pipeline or CLI invocation.
    agent_youtube::cli::run(vec!["--help".into()], std::sync::Arc::new(false.into()))
}

#[cfg(test)]
mod tests {
    use std::{
        io::Write,
        process::{Command, Stdio},
    };

    #[test]
    fn worker_fixture() {
        if std::env::var_os("AGENT_YOUTUBE_EMBED_CHILD").is_some() {
            let result = agent_youtube::dispatch_web_worker(["--internal-web-runtime".into()]);
            std::process::exit(if matches!(result, Ok(true)) { 0 } else { 1 });
        }
    }

    #[test]
    fn embedding_dispatches_private_ipc_and_leaves_other_arguments_alone() {
        for args in [
            vec![],
            vec!["--help"],
            vec!["--internal-web-runtime", "extra"],
        ] {
            assert!(!agent_youtube::dispatch_web_worker(args.into_iter().map(Into::into)).unwrap());
        }
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "tests::worker_fixture", "--nocapture"])
            .env("AGENT_YOUTUBE_EMBED_CHILD", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        // Invalid challenge is handled entirely locally, with a structured private failure.
        let mut input = child.stdin.take().unwrap();
        input
            .write_all(b"{\"operation\":\"N\",\"player\":\"\",\"challenge\":\"\"}\n")
            .unwrap();
        let player = "var namespace={};(function(g){var unused=this;function U(){this.values={}} U.prototype.set=function(k,v){this.values[k]=v}; U.prototype.get=function(k){return this.values[k]}; U.prototype.transform=function(){this.values.n+='x'}; function C(){var u=new U();u.set('alr','yes');return u} })(namespace);";
        serde_json::to_writer(
            &mut input,
            &serde_json::json!({"operation":"N","player":player,"challenge":"abc"}),
        )
        .unwrap();
        input.write_all(b"\n").unwrap();
        drop(input);
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8(output.stdout).unwrap();
        let responses: Vec<serde_json::Value> = stdout
            .lines()
            .filter(|line| line.starts_with('{'))
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(responses.len(), 2);
        assert_eq!(responses[1]["solution"], "abcx");
        let response = &responses[0];
        assert_eq!(response["failed"], true);
        assert_eq!(
            response["failure"]["code"],
            "youtube_player_challenge_failed"
        );
    }
}
