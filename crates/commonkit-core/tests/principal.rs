use std::io;

use commonkit_core::{CommandOutput, PrincipalCommandRunner, resolve_principal_with};

struct FakeRunner {
    result: Option<io::Result<CommandOutput>>,
}

impl PrincipalCommandRunner for FakeRunner {
    fn run(&mut self, arguments: &[&str]) -> io::Result<CommandOutput> {
        assert_eq!(
            arguments,
            ["api", "--hostname", "github.com", "user", "--jq", ".login"]
        );
        self.result.take().expect("one resolver invocation")
    }
}

#[test]
fn principal_resolver_fails_closed() {
    let cases = [
        (
            "valid",
            Ok(CommandOutput::success(b"Example-User\n")),
            Some("example-user"),
        ),
        (
            "unavailable",
            Err(io::Error::new(io::ErrorKind::NotFound, "missing")),
            None,
        ),
        ("signed out", Ok(CommandOutput::failure(b"")), None),
        ("malformed", Ok(CommandOutput::success([0xff])), None),
        (
            "oversized",
            Ok(CommandOutput::success(vec![b'a'; 65])),
            None,
        ),
        (
            "invalid login",
            Ok(CommandOutput::success(b"bad--login\n")),
            None,
        ),
    ];

    for (name, result, expected) in cases {
        let mut runner = FakeRunner {
            result: Some(result),
        };
        let principal = resolve_principal_with(&mut runner);
        assert_eq!(
            principal.as_ref().map(|value| value.as_str()),
            expected,
            "{name}"
        );
    }
}
