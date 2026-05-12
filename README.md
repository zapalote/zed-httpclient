# HTTP Client for Zed

Run HTTP requests directly from `.http` files in the [Zed](https://zed.dev) editor — similar to the REST Client extension for VS Code or the built-in HTTP client in JetBrains IDEs.

## Features

- **Run requests with Cmd+Click** — click the `###` separator above any request to execute it; a progress spinner shows while the request is in flight
- **Response preview tab** — response opens automatically in a syntax-highlighted tab next to your request file
- **Syntax highlighting** — methods, URLs, headers, status codes, and JSON/XML/GraphQL body injection
- **Code lens hint** — "⌘-click on ### to run" label above every request block
- Supports all standard HTTP methods: GET, POST, PUT, PATCH, DELETE, HEAD, OPTIONS
- Accepts self-signed TLS certificates (useful for local development)
- JSON responses are pretty-printed automatically

## Example

```http
### Fetch a todo
GET https://jsonplaceholder.typicode.com/todos/1
Accept: application/json

### Create a post
POST https://jsonplaceholder.typicode.com/posts
Content-Type: application/json

{
  "title": "foo",
  "body": "bar",
  "userId": 1
}
```

Cmd+Click on any `###` line to run that request. The response is written to `<filename>.response.http` and opened as a preview tab.

## Installation

### From the Zed extension marketplace

Search for **httpclient** in Zed's extension panel (`zed: extensions`), install it, then install the LSP binary:

```sh
cargo install --git https://github.com/zapalote/zed-httpclient --manifest-path lsp/Cargo.toml
```

### From source

```sh
git clone https://github.com/zapalote/zed-httpclient
cd zed-http
cargo install --path lsp        # installs zed-http-lsp to ~/.cargo/bin
```

Then in Zed, open the `zed-http` folder and run **"zed: install dev extension"** from the command palette.

## Requirements

- Rust toolchain (to build and install the `zed-http-lsp` binary)

## File format

Request files use the `.http` extension. Requests are separated by `###` lines:

```http
### Optional label
METHOD https://url
Header-Name: value

optional body
```

The response is saved as `<name>.response.http` in the same directory and opened automatically.

## Known limitations

- Variable substitution (`{{var}}` and `.env` files) is not yet implemented
- External body files (`< file.json`) are not yet supported
- HTML body syntax injection is not available in this version (grammar constraint)

## License

Apache-2.0
