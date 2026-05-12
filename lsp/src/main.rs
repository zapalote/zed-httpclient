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

        let response_path = match uri.to_file_path().ok().and_then(|p| {
            let stem = p.file_stem()?.to_string_lossy().into_owned();
            p.parent()
                .map(|dir| dir.join(format!("{}.response.http", stem)))
        }) {
            Some(p) => p,
            None => return Ok(None),
        };

        let response_uri = match Url::from_file_path(&response_path) {
            Ok(u) => u,
            Err(_) => return Ok(None),
        };

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

        let result = run_request(request).await;

        self.client
            .send_notification::<notification::Progress>(ProgressParams {
                token,
                value: ProgressParamsValue::WorkDone(WorkDoneProgress::End(WorkDoneProgressEnd {
                    message: None,
                })),
            })
            .await;

        match result {
            Ok(response_text) => {
                if let Err(e) = tokio::fs::write(&response_path, &response_text).await {
                    self.client
                        .show_message(MessageType::ERROR, format!("Could not write response: {e}"))
                        .await;
                    return Ok(None);
                }
                Ok(Some(GotoDefinitionResponse::Scalar(Location {
                    uri: response_uri,
                    range: Range::default(),
                })))
            }
            Err(e) => {
                self.client
                    .show_message(MessageType::ERROR, format!("Request failed: {e:#}"))
                    .await;
                Ok(None)
            }
        }
    }

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

    let mut out = format!(
        "{} {} HTTP/1.1\n{} {} {}\n",
        req.method,
        req.url,
        version,
        status.as_u16(),
        status.canonical_reason().unwrap_or("Unknown"),
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
