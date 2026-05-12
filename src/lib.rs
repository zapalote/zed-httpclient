use zed_extension_api::{self as zed, Command, LanguageServerId, Result, Worktree};

struct HttpClientExtension;

impl zed::Extension for HttpClientExtension {
    fn new() -> Self {
        Self
    }

    fn language_server_command(
        &mut self,
        _language_server_id: &LanguageServerId,
        worktree: &Worktree,
    ) -> Result<Command> {
        // worktree.which() searches the PATH visible inside the WASM sandbox.
        // If that comes up empty, fall back to the standard cargo install location.
        let binary = worktree.which("zed-http-lsp").or_else(|| {
            worktree
                .shell_env()
                .into_iter()
                .find(|(k, _)| k == "HOME")
                .map(|(_, home)| format!("{}/.cargo/bin/zed-http-lsp", home))
        });

        match binary {
            Some(path) => Ok(Command {
                command: path,
                args: vec![],
                env: vec![],
            }),
            None => Err("zed-http-lsp not found — run: cargo install --path lsp".into()),
        }
    }
}

zed::register_extension!(HttpClientExtension);
