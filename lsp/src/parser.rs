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

/// Parses the single request block that begins after `separator_line` (a `###` line).
/// The block ends at the next `###` or EOF.
pub fn parse_request_at_line(content: &str, separator_line: u32) -> Option<HttpRequest> {
    let lines: Vec<&str> = content.lines().collect();
    let block_start = (separator_line as usize) + 1;

    if block_start >= lines.len() {
        return None;
    }

    let block_end = lines[block_start..]
        .iter()
        .position(|l| l.starts_with("###"))
        .map(|p| block_start + p)
        .unwrap_or(lines.len());

    let block = &lines[block_start..block_end];

    // First non-blank line is METHOD URL
    let mut iter = block
        .iter()
        .enumerate()
        .skip_while(|(_, l)| l.trim().is_empty());

    let (_, first_line) = iter.next()?;
    let mut parts = first_line.split_whitespace();
    let method = parts.next()?.to_uppercase();
    let url = parts.next()?.to_string();

    // Header lines come next, until an empty line
    let mut headers: Vec<(String, String)> = Vec::new();
    let mut body_lines: Option<Vec<&str>> = None;

    for (i, line) in iter {
        if line.trim().is_empty() {
            // Everything after this blank line is the body
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
        // Trim trailing blank lines
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
}
