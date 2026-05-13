//! LSP server for `.http` and `.rest` files.
//!
//! Implements two LSP features:
//! - **Code lens**: shows a "⌘-click on ### to send request" hint above every `###` separator.
//! - **Go to definition**: triggered by Cmd+Click on a `###` line — executes the HTTP request,
//!   writes the response to `<filename>.response.http`, and returns that file as the definition
//!   location so Zed opens it in a preview tab.
//!
//! Variable substitution (`{{VAR}}`) is resolved from a `.env` file in the same directory as
//! the `.http` file, read fresh on every request execution (avoid stale caches).

mod parser;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use serde_json::Value;
use tokio::sync::RwLock;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};

/// Shared server state. `documents` mirrors the content of every open `.http` file
/// so that code lens and go-to-definition can work without disk reads.
struct Backend {
    client: Client,
    documents: Arc<RwLock<HashMap<Url, String>>>,
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, _params: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                // Full-document sync: the client sends the entire file on every change.
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                // Code lens: decorative hint above each ### separator.
                code_lens_provider: Some(CodeLensOptions {
                    resolve_provider: Some(false),
                }),
                // Definition: the mechanism used to run requests and open the response tab.
                definition_provider: Some(OneOf::Left(true)),
                ..Default::default()
            },
            server_info: Some(ServerInfo {
                name: "zed-http-lsp".to_string(),
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
            }),
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "zed-http-lsp ready")
            .await;
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        self.documents
            .write()
            .await
            .insert(params.text_document.uri, params.text_document.text);
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        if let Some(change) = params.content_changes.into_iter().last() {
            self.documents
                .write()
                .await
                .insert(params.text_document.uri, change.text);
        }
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        self.documents
            .write()
            .await
            .remove(&params.text_document.uri);
    }

    /// "Go to request" — conceptually this is `goto_request`, but we implement it as
    /// `goto_definition` because that is the only LSP call that causes Zed to open a
    /// file in a preview tab (the same mechanism Zed uses for its own code navigation).
    ///
    /// Only activates when the cursor is on a `###` separator line. Parses the request
    /// block that follows, substitutes `.env` variables, executes the HTTP request, writes
    /// the response to `<stem>.response.http` next to the source file, and returns that
    /// file as the definition location — which causes Zed to open it in a preview tab.
    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let uri = params
            .text_document_position_params
            .text_document
            .uri
            .clone();
        let line = params.text_document_position_params.position.line;

        let docs = self.documents.read().await;
        let content = match docs.get(&uri) {
            Some(c) => c.clone(),
            None => return Ok(None),
        };
        drop(docs);

        // Only act on ### separator lines; ignore clicks on request body, headers, etc.
        if !content
            .lines()
            .nth(line as usize)
            .unwrap_or("")
            .starts_with("###")
        {
            return Ok(None);
        }

        let request = match parser::parse_request_at_line(&content, line) {
            Some(r) => r,
            None => {
                self.client
                    .show_message(MessageType::ERROR, "Could not parse HTTP request block")
                    .await;
                return Ok(None);
            }
        };

        let file_path = match uri.to_file_path().ok() {
            Some(p) => p,
            None => return Ok(None),
        };
        let http_dir = match file_path.parent() {
            Some(d) => d.to_path_buf(),
            None => return Ok(None),
        };
        let stem = match file_path.file_stem() {
            Some(s) => s.to_string_lossy().into_owned(),
            None => return Ok(None),
        };

        // env file resolution (in priority order):
        // 1. # ENV=path declaration in the .http file
        // 2. <stem>.env in the same directory
        // 3. .env in the same directory
        let env_filepath = parser::find_env_filepath(&content)
            .map(|s| {
                let p = PathBuf::from(s);
                if p.is_absolute() {
                    p
                } else {
                    http_dir.join(p)
                }
            })
            .or_else(|| {
                let p = http_dir.join(format!("{}.env", stem));
                p.exists().then_some(p)
            })
            .or_else(|| {
                let p = http_dir.join(".env");
                p.exists().then_some(p)
            });
        // Read env file fresh on every execution so edits take effect immediately.
        let env_vars = match env_filepath {
            Some(path) => tokio::fs::read_to_string(path)
                .await
                .map(|s| parser::load_env(&s))
                .unwrap_or_default(),
            None => HashMap::new(),
        };
        // and apply them to the request (uri, headers, body)
        let request = parser::apply_vars(request, &env_vars);

        // create response path and uri
        let response_path = http_dir.join(format!("{}.response.http", stem));
        let response_uri = match Url::from_file_path(&response_path) {
            Ok(u) => u,
            Err(_) => return Ok(None),
        };

        // Show a progress spinner in Zed while the request is in flight.
        let token = NumberOrString::String(format!("http-{line}"));
        let _ = self
            .client
            .send_request::<request::WorkDoneProgressCreate>(WorkDoneProgressCreateParams {
                token: token.clone(),
            })
            .await;
        self.client
            .send_notification::<notification::Progress>(ProgressParams {
                token: token.clone(),
                value: ProgressParamsValue::WorkDone(WorkDoneProgress::Begin(
                    WorkDoneProgressBegin {
                        title: format!("{} {}", request.method, request.url),
                        cancellable: Some(false),
                        message: None,
                        percentage: None,
                    },
                )),
            })
            .await;

        // send the request and get the response
        let result = run_request(request).await;

        // stop the spinner (indicate request is complete)
        self.client
            .send_notification::<notification::Progress>(ProgressParams {
                token,
                value: ProgressParamsValue::WorkDone(WorkDoneProgress::End(WorkDoneProgressEnd {
                    message: None,
                })),
            })
            .await;

        // write the response to the response path
        match result {
            Ok(response_text) => {
                if let Err(e) = tokio::fs::write(&response_path, &response_text).await {
                    self.client
                        .show_message(MessageType::ERROR, format!("Could not write response: {e}"))
                        .await;
                    return Ok(None);
                }
                // Returning a Location causes Zed to open the file in a preview tab.
                // We use Zed's built-in go_to_definition response type to do this.
                Ok(Some(GotoDefinitionResponse::Scalar(Location {
                    uri: response_uri,
                    range: Range::default(),
                })))
            }
            // or show an error message if the request fails
            Err(e) => {
                self.client
                    .show_message(MessageType::ERROR, format!("Request failed: {e:#}"))
                    .await;
                Ok(None)
            }
        }
    }

    /// Returns a non-clickable code lens hint above every `###` separator line.
    async fn code_lens(&self, params: CodeLensParams) -> Result<Option<Vec<CodeLens>>> {
        let uri = params.text_document.uri.clone();
        let docs = self.documents.read().await;
        let content = match docs.get(&uri) {
            Some(c) => c.clone(),
            None => return Ok(Some(vec![])),
        };
        drop(docs);

        let lenses = parser::find_separator_lines(&content)
            .into_iter()
            .map(|line| CodeLens {
                range: Range {
                    start: Position { line, character: 0 },
                    end: Position { line, character: 3 },
                },
                command: Some(Command {
                    title: "⌘-click on ### to send request".to_string(),
                    command: String::new(),
                    arguments: None,
                }),
                data: None,
            })
            .collect();

        Ok(Some(lenses))
    }
}

/// Executes an HTTP request and formats the response as a `.response.http` file.
///
/// The output starts with a fake `METHOD URL HTTP/1.1` request line so that the
/// tree-sitter grammar (pinned to an older commit) parses the file as a `request`
/// node containing an inline `response`, enabling full syntax highlighting.
///
/// JSON responses are pretty-printed. Invalid TLS certificates are accepted to
/// support local development servers.
async fn run_request(req: parser::HttpRequest) -> anyhow::Result<String> {
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .context("building HTTP client")?;

    let method =
        reqwest::Method::from_bytes(req.method.as_bytes()).context("invalid HTTP method")?;

    let mut builder = client.request(method, &req.url);
    for (name, value) in &req.headers {
        builder = builder.header(name.as_str(), value.as_str());
    }
    if let Some(body) = req.body {
        builder = builder.body(body);
    }

    let response = builder.send().await.context("sending request")?;
    let status = response.status();
    let version = format!("{:?}", response.version());
    let headers = response.headers().clone();
    let body_bytes = response.bytes().await.context("reading response body")?;

    // Fake request line makes the grammar parse this as a valid for syntax highlighting.
    let mut out = format!(
        "{} {} HTTP/1.1\n{} {} {}\n",
        req.method,
        req.url,
        version,
        status.as_u16(),
        status.canonical_reason().unwrap_or("Unknown"),
    );

    // add response headers to the output
    for (name, value) in &headers {
        if let Ok(v) = value.to_str() {
            out.push_str(&format!("{}: {}\n", name, v));
        }
    }
    out.push('\n');

    // check if the response is JSON and format it accordingly
    let is_json = headers
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.contains("application/json"))
        .unwrap_or(false);

    if is_json {
        if let Ok(json) = serde_json::from_slice::<Value>(&body_bytes) {
            out.push_str(&serde_json::to_string_pretty(&json).unwrap_or_default());
        } else {
            out.push_str(&String::from_utf8_lossy(&body_bytes));
        }
    } else {
        out.push_str(&String::from_utf8_lossy(&body_bytes));
    }

    // ship it
    Ok(out)
}

#[tokio::main]
async fn main() {
    // start the LSP server
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(|client| Backend {
        client,
        documents: Arc::new(RwLock::new(HashMap::new())),
    });

    Server::new(stdin, stdout, socket).serve(service).await;
}
