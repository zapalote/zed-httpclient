//! Parser for `.http` / `.rest` request files.
//!
//! File format:
//! ```text
//! ### Optional label
//! METHOD https://url
//! Header-Name: value
//!
//! optional body
//!
//! ### Next request
//! ...
//! ```
//!
//! Requests are separated by lines starting with `###`. Each block contains one
//! HTTP request: a method+URL line, optional headers, an optional blank-line-delimited body.
//! Variable placeholders (`{{VAR}}`) are substituted from a `.env` file at execution time.

use std::collections::HashMap;

/// A parsed HTTP request, ready to execute.
#[derive(Debug, Clone)]
pub struct HttpRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
}

/// Returns the 0-based line numbers of every `###` separator in the file.
pub fn find_separator_lines(content: &str) -> Vec<u32> {
    content
        .lines()
        .enumerate()
        .filter(|(_, line)| line.starts_with("###"))
        .map(|(i, _)| i as u32)
        .collect()
}

/// Parses the request block that begins immediately after `separator_line` (a `###` line).
///
/// The block ends at the next `###` or EOF. Returns `None` if the block is empty or
/// does not contain a valid `METHOD URL` line.
pub fn parse_request_at_line(content: &str, separator_line: u32) -> Option<HttpRequest> {
    let lines: Vec<&str> = content.lines().collect();
    let block_start = (separator_line as usize) + 1;

    if block_start >= lines.len() {
        return None;
    }

    // Block ends at the next ### separator or EOF.
    let block_end = lines[block_start..]
        .iter()
        .position(|l| l.starts_with("###"))
        .map(|p| block_start + p)
        .unwrap_or(lines.len());

    let block = &lines[block_start..block_end];

    // First non-blank line must be `METHOD URL`.
    let mut iter = block
        .iter()
        .enumerate()
        .skip_while(|(_, l)| l.trim().is_empty());

    let (_, first_line) = iter.next()?;
    let mut parts = first_line.split_whitespace();
    let method = parts.next()?.to_uppercase();
    let url = parts.next()?.to_string();

    // Subsequent non-blank lines are `Name: value` headers until the first blank line.
    let mut headers: Vec<(String, String)> = Vec::new();
    let mut body_lines: Option<Vec<&str>> = None;

    for (i, line) in iter {
        if line.trim().is_empty() {
            // Everything after this blank line is the body.
            let rest = &block[i + 1..];
            if !rest.iter().all(|l| l.trim().is_empty()) {
                body_lines = Some(rest.to_vec());
            }
            break;
        }
        if let Some(colon) = line.find(':') {
            let name = line[..colon].trim().to_string();
            let value = line[colon + 1..].trim().to_string();
            headers.push((name, value));
        }
    }

    let body = body_lines.map(|lines| {
        // Trim trailing blank lines from the body.
        let trimmed: Vec<&str> = {
            let mut v = lines;
            while v.last().map(|l: &&str| l.trim().is_empty()).unwrap_or(false) {
                v.pop();
            }
            v
        };
        trimmed.join("\n")
    });

    Some(HttpRequest {
        method,
        url,
        headers,
        body,
    })
}

/// Parses a `.env` file into a variable map.
///
/// Supports `KEY=value`, `KEY="value"`, `KEY='value'`, blank lines, and `#` comments.
pub fn load_env(content: &str) -> HashMap<String, String> {
    content
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
        .filter_map(|l| {
            let (key, val) = l.split_once('=')?;
            let key = key.trim().to_string();
            let val = val.trim().trim_matches('"').trim_matches('\'').to_string();
            Some((key, val))
        })
        .collect()
}

/// Replaces `{{VAR}}` with the value from `vars` in a single string.
fn substitute(s: &str, vars: &HashMap<String, String>) -> String {
    let mut result = s.to_string();
    for (k, v) in vars {
        result = result.replace(&format!("{{{{{}}}}}", k), v);
    }
    result
}

/// Removes double slashes from the path part of a URL after variable substitution,
/// e.g. when `BASE_URL` ends with `/` and the template has a leading `/`.
/// The `://` scheme separator is left intact.
fn normalize_url(url: String) -> String {
    if let Some(pos) = url.find("://") {
        let (scheme, rest) = url.split_at(pos + 3);
        format!("{}{}", scheme, rest.replace("//", "/"))
    } else {
        url.replace("//", "/")
    }
}

/// Substitutes `{{VAR}}` placeholders in all fields of a request using the provided map.
pub fn apply_vars(req: HttpRequest, vars: &HashMap<String, String>) -> HttpRequest {
    if vars.is_empty() {
        return req;
    }
    HttpRequest {
        method: substitute(&req.method, vars),
        url: normalize_url(substitute(&req.url, vars)),
        headers: req
            .headers
            .into_iter()
            .map(|(k, v)| (substitute(&k, vars), substitute(&v, vars)))
            .collect(),
        body: req.body.map(|b| substitute(&b, vars)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
### Simple GET
GET http://example.com/api/data
Accept: application/json

###
POST http://example.com/api/users
Content-Type: application/json
Authorization: Bearer token123

{
  "name": "John"
}

### End
"#;

    #[test]
    fn finds_separator_lines() {
        let lines = find_separator_lines(SAMPLE);
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn parses_get_request() {
        let lines = find_separator_lines(SAMPLE);
        let req = parse_request_at_line(SAMPLE, lines[0]).unwrap();
        assert_eq!(req.method, "GET");
        assert_eq!(req.url, "http://example.com/api/data");
        assert_eq!(req.headers, vec![("Accept".to_string(), "application/json".to_string())]);
        assert!(req.body.is_none());
    }

    #[test]
    fn parses_post_request_with_body() {
        let lines = find_separator_lines(SAMPLE);
        let req = parse_request_at_line(SAMPLE, lines[1]).unwrap();
        assert_eq!(req.method, "POST");
        assert_eq!(req.url, "http://example.com/api/users");
        assert_eq!(req.headers.len(), 2);
        assert!(req.body.is_some());
        assert!(req.body.unwrap().contains("\"name\""));
    }

    #[test]
    fn loads_env_file() {
        let env = load_env("HOST=https://api.example.com\n# comment\nTOKEN=\"abc123\"\n\nEMPTY=");
        assert_eq!(env["HOST"], "https://api.example.com");
        assert_eq!(env["TOKEN"], "abc123");
        assert_eq!(env["EMPTY"], "");
    }

    #[test]
    fn substitutes_vars_in_request() {
        let vars = HashMap::from([
            ("HOST".to_string(), "https://api.example.com".to_string()),
            ("TOKEN".to_string(), "secret".to_string()),
        ]);
        let req = HttpRequest {
            method: "GET".to_string(),
            url: "{{HOST}}/users".to_string(),
            headers: vec![("Authorization".to_string(), "Bearer {{TOKEN}}".to_string())],
            body: None,
        };
        let req = apply_vars(req, &vars);
        assert_eq!(req.url, "https://api.example.com/users");
        assert_eq!(req.headers[0].1, "Bearer secret");
    }

    #[test]
    fn normalizes_double_slash_after_substitution() {
        let vars = HashMap::from([("BASE_URL".to_string(), "https://api.example.com/".to_string())]);
        let req = HttpRequest {
            method: "GET".to_string(),
            url: "{{BASE_URL}}/users".to_string(),
            headers: vec![],
            body: None,
        };
        let req = apply_vars(req, &vars);
        assert_eq!(req.url, "https://api.example.com/users");
    }
}
