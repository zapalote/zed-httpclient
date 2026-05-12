; Methods
(method) @function.method

; Comments
(comment) @comment

; URLs
(target_url) @string.special.url

; Headers
(header name: (name) @property)
(header value: (value) @string)

; Status
(status_code) @number
(status_text) @string

; HTTP version
(http_version) @constant

; Variables
(variable) @variable
(variable_declaration) @variable

; Bodies
(json_body) @string
(xml_body) @string
(graphql_body) @string
(external_body) @string
