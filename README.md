# HTTP Client for Zed

Run HTTP requests directly from `.http` or `.rest` files in the [Zed](https://zed.dev) editor — similar to the REST Client extension for VS Code or the built-in HTTP client in JetBrains IDEs.

## Features

- **Run requests with Cmd+Click** — click the `###` separator above any request to execute it; a progress spinner shows while the request is in flight
- **Response preview tab** — response opens automatically in a syntax-highlighted tab next to your request file
- **Syntax highlighting** — methods, URLs, headers, status codes, and JSON/XML/GraphQL body injection
- **Code lens hint** — "⌘-click on ### to send request" label above every request block
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

## File format

Request files use the `.http` or `.rest` extension. Requests are separated by (start with) `###` lines:

```http
### Optional label
METHOD https://url
Header-Name: value

optional body
```

The response is saved as `<name>.response.http` in the same directory and opened automatically.

## Variables

Variables can be defined in `.env` files (see [dotenv](https://crates.io/crates/dotenv)).
This is essential to avoid cluttering test files with sensitive information.

```
BASE_URL=https://example.com
TOKEN=your_secret_token
```

Variables are substituted in requests using the `{{var}}` syntax.

```
### Get user
GET {{BASE_URL}}/users/1
Authorization: Bearer {{TOKEN}}

```

## Installation

### Zed extension marketplace

This extension is not available on the Zed extension marketplace. The reason being that the lsp-server that does the parsing and executing of HTTP requests is a standalone binary that has to be compiled for different platforms (MacOs, Windows, Linux). I lack the resources and the time to maintain that binary for all platforms.

### Install from source

```sh
git clone https://github.com/zapalote/zed-httpclient
cd zed-httpclient
cargo install --path lsp        # installs zed-http-lsp to ~/.cargo/bin
```

Then in Zed, open the `zed-httpclient` folder and run **"zed: install dev extension"** from the command palette (Ctrl+Shift+P or Cmd+Shift+P).

### Requirements

- Rust toolchain (to build and install the `zed-http-lsp` binary). See [rustup](https://rustup.rs/) for installation instructions.

## Known limitations

- no support for indicating the HTTP version, only HTTP/1.1 supported
- HTML body syntax injection is not available in this version (grammar constraint)

## License

Apache-2.0
