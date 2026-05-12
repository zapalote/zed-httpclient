mod parser;

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Context;
use serde_json::Value;
use tokio::sync::RwLock;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};

struct Backend {
    client: Client,
    documents: Arc<RwLock<HashMap<Url, String>>>,
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, _params: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                code_lens_provider: Some(CodeLensOptions {
                    resolve_provider: Some(false),
                }),
                code_action_provider: Some(CodeActionProviderCapability::Simple(true)),
                execute_command_provider: Some(ExecuteCommandOptions {
                    commands: vec!["http.runRequest".to_string()],
                    work_done_progress_options: Default::default(),
                }),
                ..Default::default()
            },
            server_info: Some(ServerInfo {
                name: "zed-http-lsp".to_string(),
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
            }),
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        eprintln!("[http-lsp] initialized");
        self.client
            .log_message(MessageType::INFO, "zed-http-lsp ready")
            .await;
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        eprintln!("[http-lsp] did_open: {}", params.text_document.uri);
        eprintln!("[http-lsp] did_open content length: {}", params.text_document.text.len());
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

    async fn code_action(&self, params: CodeActionParams) -> Result<Option<CodeActionResponse>> {
        eprintln!("[http-lsp] code_action called, line: {}", params.range.start.line);
        let line = params.range.start.line;
        let uri = params.text_document.uri.clone();

        let docs = self.documents.read().await;
        let content = match docs.get(&uri) {
            Some(c) => c.clone(),
            None => {
                eprintln!("[http-lsp] code_action: document not in store, uri={uri}");
                return Ok(None);
            }
        };
        drop(docs);

        let line_text = content.lines().nth(line as usize).unwrap_or("");
        eprintln!("[http-lsp] code_action: line {line} = {line_text:?}");

        let is_separator = line_text.starts_with("###");
        if !is_separator {
            return Ok(None);
        }

        let action = CodeActionOrCommand::CodeAction(CodeAction {
            title: "▶ Run HTTP Request".to_string(),
            kind: Some(CodeActionKind::EMPTY),
            command: Some(Command {
                title: "▶ Run HTTP Request".to_string(),
                command: "http.runRequest".to_string(),
                arguments: Some(vec![
                    Value::String(uri.to_string()),
                    Value::Number(line.into()),
                ]),
            }),
            ..Default::default()
        });

        Ok(Some(vec![action]))
    }

    async fn code_lens(&self, params: CodeLensParams) -> Result<Option<Vec<CodeLens>>> {
        eprintln!("[http-lsp] code_lens request for: {}", params.text_document.uri);
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
                    start: Position {
                        line,
                        character: 0,
                    },
                    end: Position {
                        line,
                        character: 3,
                    },
                },
                command: Some(Command {
                    title: "▶ Run".to_string(),
                    command: "http.runRequest".to_string(),
                    arguments: Some(vec![
                        Value::String(uri.to_string()),
                        Value::Number(line.into()),
                    ]),
                }),
                data: None,
            })
            .collect();

        Ok(Some(lenses))
    }

    async fn execute_command(&self, params: ExecuteCommandParams) -> Result<Option<Value>> {
        if params.command != "http.runRequest" {
            return Ok(None);
        }

        let (uri_str, separator_line) = match parse_command_args(&params.arguments) {
            Some(v) => v,
            None => {
                self.client
                    .show_message(MessageType::ERROR, "Invalid http.runRequest arguments")
                    .await;
                return Ok(None);
            }
        };

        let uri: Url = match uri_str.parse() {
            Ok(u) => u,
            Err(_) => {
                self.client
                    .show_message(MessageType::ERROR, format!("Invalid URI: {uri_str}"))
                    .await;
                return Ok(None);
            }
        };

        let docs = self.documents.read().await;
        let content = match docs.get(&uri) {
            Some(c) => c.clone(),
            None => {
                self.client
                    .show_message(MessageType::ERROR, "Document not found in server state")
                    .await;
                return Ok(None);
            }
        };
        drop(docs);

        let request = match parser::parse_request_at_line(&content, separator_line) {
            Some(r) => r,
            None => {
                self.client
                    .show_message(MessageType::ERROR, "Could not parse HTTP request block")
                    .await;
                return Ok(None);
            }
        };

        self.client
            .show_message(
                MessageType::INFO,
                format!("Running {} {}", request.method, request.url),
            )
            .await;

        // Derive the sidecar response file URI: foo.http → foo.response.txt
        let response_uri = uri.to_file_path().ok()
            .and_then(|p| {
                let stem = p.file_stem()?.to_string_lossy().into_owned();
                let response_name = format!("{}.response.txt", stem);
                p.parent().map(|dir| dir.join(response_name))
            })
            .and_then(|p| Url::from_file_path(p).ok());

        let response_uri = match response_uri {
            Some(u) => u,
            None => {
                self.client
                    .show_message(MessageType::ERROR, "Could not derive response file path")
                    .await;
                return Ok(None);
            }
        };

        let client = self.client.clone();
        tokio::spawn(async move {
            eprintln!("[http-lsp] spawned task: running request");
            match run_request(request).await {
                Ok(response_text) => {
                    eprintln!("[http-lsp] request succeeded, response len={}", response_text.len());

                    // Create-or-overwrite the sidecar file, then fill it with the response.
                    let edit = WorkspaceEdit {
                        document_changes: Some(DocumentChanges::Operations(vec![
                            DocumentChangeOperation::Op(ResourceOp::Create(CreateFile {
                                uri: response_uri.clone(),
                                options: Some(CreateFileOptions {
                                    overwrite: Some(true),
                                    ignore_if_exists: Some(false),
                                }),
                                annotation_id: None,
                            })),
                            DocumentChangeOperation::Edit(TextDocumentEdit {
                                text_document: OptionalVersionedTextDocumentIdentifier {
                                    uri: response_uri.clone(),
                                    version: None,
                                },
                                edits: vec![OneOf::Left(TextEdit {
                                    range: Range::default(),
                                    new_text: response_text,
                                })],
                            }),
                        ])),
                        ..Default::default()
                    };

                    match client.apply_edit(edit).await {
                        Ok(r) => {
                            eprintln!("[http-lsp] apply_edit ok, applied={}", r.applied);
                            if !r.applied {
                                client
                                    .show_message(MessageType::WARNING,
                                        format!("Response written — open {} to view it", response_uri.path()))
                                    .await;
                            }
                        }
                        Err(e) => {
                            eprintln!("[http-lsp] apply_edit failed: {e}");
                            client
                                .show_message(MessageType::ERROR, format!("Could not write response: {e}"))
                                .await;
                        }
                    }
                }
                Err(e) => {
                    eprintln!("[http-lsp] request failed: {e:#}");
                    client
                        .show_message(MessageType::ERROR, format!("Request failed: {e:#}"))
                        .await;
                }
            }
        });

        Ok(None)
    }
}

fn parse_command_args(args: &[Value]) -> Option<(String, u32)> {
    let uri = args.first()?.as_str()?.to_string();
    let line = args.get(1)?.as_u64()? as u32;
    Some((uri, line))
}

async fn run_request(req: parser::HttpRequest) -> anyhow::Result<String> {
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .context("building HTTP client")?;

    let method = reqwest::Method::from_bytes(req.method.as_bytes())
        .context("invalid HTTP method")?;

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

    let mut out = format!(
        "{} {} {}\n",
        version,
        status.as_u16(),
        status.canonical_reason().unwrap_or("")
    );

    for (name, value) in &headers {
        if let Ok(v) = value.to_str() {
            out.push_str(&format!("{}: {}\n", name, v));
        }
    }

    out.push('\n');

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

    Ok(out)
}

#[tokio::main]
async fn main() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(|client| Backend {
        client,
        documents: Arc::new(RwLock::new(HashMap::new())),
    });

    Server::new(stdin, stdout, socket).serve(service).await;
}
