use std::{
    collections::BTreeMap,
    fmt,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use ureq::{Agent, Body, http::Response};

use crate::release::LocalFile;

const MAX_RESPONSE_BYTES: u64 = 1 << 20;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(300);
const UNPROCESSABLE: u16 = 422;
const USER_AGENT: &str = concat!("ceagle/", env!("CARGO_PKG_VERSION"));

#[derive(Debug)]
pub enum ApiError {
    Network(String),
    Refused { status: u16, message: String },
    Problems(Vec<String>),
    Invalid(String),
}

impl fmt::Display for ApiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApiError::Network(error) => write!(formatter, "{error}"),
            ApiError::Refused { status, message } => write!(formatter, "{message} (HTTP {status})"),
            ApiError::Problems(problems) => write!(formatter, "{}", problems.join(", ")),
            ApiError::Invalid(error) => write!(
                formatter,
                "the server sent a response ceagle does not understand: {error}"
            ),
        }
    }
}

impl std::error::Error for ApiError {}

#[derive(Debug, Deserialize)]
pub struct ReleaseResponse {
    pub version: String,
    pub status: String,
    pub capabilities: Option<Vec<String>>,
    pub reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct DryRunFile {
    pub filename: String,
    pub role: String,
}

#[derive(Debug, Deserialize)]
pub struct DryRunDependency {
    pub slug: String,
    pub min_version: Option<String>,
    pub min_libload_version: Option<i64>,
    pub auto: bool,
}

#[derive(Debug, Deserialize)]
pub struct DryRunResponse {
    pub version: String,
    pub files: Vec<DryRunFile>,
    pub dependencies: Vec<DryRunDependency>,
    pub problems: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct ErrorResponse {
    error: Option<String>,
    problems: Option<Vec<String>>,
}

#[derive(Serialize)]
struct FileSetting<'a> {
    filename: &'a str,
    role: &'a str,
    description: &'a str,
}

#[derive(Serialize)]
struct Manifest<'a> {
    version: &'a str,
    notes: &'a str,
    files: Vec<FileSetting<'a>>,
    dependencies: &'a BTreeMap<String, String>,
}

#[derive(Serialize)]
struct ExchangeRequest<'a> {
    token: &'a str,
    project: &'a str,
}

#[derive(Deserialize)]
struct ExchangeResponse {
    token: String,
}

#[derive(Deserialize)]
struct GitHubTokenResponse {
    value: String,
}

pub struct Upload<'a> {
    pub version: &'a str,
    pub notes: &'a str,
    pub files: &'a [LocalFile],
    pub dependencies: &'a BTreeMap<String, String>,
    pub attestation: Option<&'a [u8]>,
}

pub struct Client {
    base: String,
    agent: Agent,
}

fn multipart(
    manifest: &[u8],
    files: &[LocalFile],
    attestation: Option<&[u8]>,
) -> (String, Vec<u8>) {
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let mut attempt = 0u32;
    let boundary = loop {
        let candidate = format!("ceagle-{seed:x}-{attempt}");
        let used = |content: &[u8]| {
            content
                .windows(candidate.len())
                .any(|window| window == candidate.as_bytes())
        };
        if !used(manifest)
            && !files.iter().any(|file| used(&file.content))
            && !attestation.is_some_and(used)
        {
            break candidate;
        }
        attempt += 1;
    };
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"release\"\r\nContent-Type: application/json\r\n\r\n").as_bytes());
    body.extend_from_slice(manifest);
    for file in files {
        body.extend_from_slice(format!("\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{}\"\r\nContent-Type: application/octet-stream\r\n\r\n", file.filename).as_bytes());
        body.extend_from_slice(&file.content);
    }
    if let Some(attestation) = attestation {
        body.extend_from_slice(format!("\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"attestation\"; filename=\"attestation.sigstore.json\"\r\nContent-Type: application/json\r\n\r\n").as_bytes());
        body.extend_from_slice(attestation);
    }
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (boundary, body)
}

impl Client {
    pub fn new(base: &str) -> Client {
        let agent = Agent::config_builder()
            .http_status_as_error(false)
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .timeout_global(Some(REQUEST_TIMEOUT))
            .user_agent(USER_AGENT)
            .build()
            .into();
        Client {
            base: base.trim_end_matches('/').to_string(),
            agent,
        }
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    fn decode<T: DeserializeOwned>(
        &self,
        result: Result<Response<Body>, ureq::Error>,
    ) -> Result<T, ApiError> {
        let mut response = result.map_err(|error| {
            ApiError::Network(format!("could not reach {}: {error}", self.base))
        })?;
        let status = response.status().as_u16();
        let body = response
            .body_mut()
            .with_config()
            .limit(MAX_RESPONSE_BYTES)
            .read_to_vec()
            .map_err(|error| {
                ApiError::Network(format!(
                    "failed to read the answer from {}: {error}",
                    self.base
                ))
            })?;
        if (200..300).contains(&status) {
            return serde_json::from_slice(&body)
                .map_err(|error| ApiError::Invalid(error.to_string()));
        }
        let failure: Option<ErrorResponse> = serde_json::from_slice(&body).ok();
        let (message, problems) =
            failure.map_or((None, None), |failure| (failure.error, failure.problems));
        match problems {
            Some(problems) if status == UNPROCESSABLE && !problems.is_empty() => {
                Err(ApiError::Problems(problems))
            }
            _ => Err(ApiError::Refused {
                status,
                message: message
                    .unwrap_or_else(|| format!("{} answered without an error message", self.base)),
            }),
        }
    }

    fn send_release<T: DeserializeOwned>(
        &self,
        slug: &str,
        token: &str,
        upload: &Upload,
        query: &str,
    ) -> Result<T, ApiError> {
        let manifest = Manifest {
            version: upload.version,
            notes: upload.notes,
            files: upload
                .files
                .iter()
                .map(|file| FileSetting {
                    filename: &file.filename,
                    role: &file.role,
                    description: &file.description,
                })
                .collect(),
            dependencies: upload.dependencies,
        };
        let manifest =
            serde_json::to_vec(&manifest).map_err(|error| ApiError::Invalid(error.to_string()))?;
        let (boundary, body) = multipart(&manifest, upload.files, upload.attestation);
        let result = self
            .agent
            .post(format!(
                "{}/api/v1/projects/{slug}/releases{query}",
                self.base
            ))
            .header("Authorization", format!("Bearer {token}"))
            .header(
                "Content-Type",
                format!("multipart/form-data; boundary={boundary}"),
            )
            .send(&body[..]);
        self.decode(result)
    }

    pub fn dry_run(
        &self,
        slug: &str,
        token: &str,
        upload: &Upload,
    ) -> Result<DryRunResponse, ApiError> {
        self.send_release(slug, token, upload, "?dry_run=1")
    }

    pub fn publish(
        &self,
        slug: &str,
        token: &str,
        upload: &Upload,
    ) -> Result<ReleaseResponse, ApiError> {
        self.send_release(slug, token, upload, "")
    }

    pub fn release(
        &self,
        slug: &str,
        version: &str,
        token: &str,
    ) -> Result<ReleaseResponse, ApiError> {
        let result = self
            .agent
            .get(format!(
                "{}/api/v1/projects/{slug}/releases/{version}",
                self.base
            ))
            .header("Authorization", format!("Bearer {token}"))
            .call();
        self.decode(result)
    }

    pub fn exchange(&self, github_token: &str, project: &str) -> Result<String, ApiError> {
        let body = serde_json::to_vec(&ExchangeRequest {
            token: github_token,
            project,
        })
        .map_err(|error| ApiError::Invalid(error.to_string()))?;
        let result = self
            .agent
            .post(format!("{}/api/v1/oidc/github/exchange", self.base))
            .header("Content-Type", "application/json")
            .send(&body[..]);
        self.decode::<ExchangeResponse>(result)
            .map(|response| response.token)
    }

    pub fn github_token(
        &self,
        request_url: &str,
        request_token: &str,
        audience: &str,
    ) -> Result<String, ApiError> {
        let separator = if request_url.contains('?') { '&' } else { '?' };
        let result = self
            .agent
            .get(format!("{request_url}{separator}audience={audience}"))
            .header("Authorization", format!("bearer {request_token}"))
            .call();
        let mut response = result.map_err(|error| {
            ApiError::Network(format!("could not reach GitHub for an OIDC token: {error}"))
        })?;
        let status = response.status().as_u16();
        let body = response
            .body_mut()
            .with_config()
            .limit(MAX_RESPONSE_BYTES)
            .read_to_vec()
            .map_err(|error| {
                ApiError::Network(format!(
                    "failed to read the OIDC token from GitHub: {error}"
                ))
            })?;
        if status != 200 {
            return Err(ApiError::Refused { status, message: "GitHub did not give an OIDC token, check that the job has permissions: id-token: write".into() });
        }
        let token: GitHubTokenResponse =
            serde_json::from_slice(&body).map_err(|error| ApiError::Invalid(error.to_string()))?;
        Ok(token.value)
    }
}

#[cfg(test)]
pub mod fake {
    use std::{
        io::{BufRead, BufReader, Read, Write},
        net::TcpListener,
        thread::{self, JoinHandle},
    };

    pub struct Request {
        pub head: String,
        pub body: Vec<u8>,
    }

    pub fn serve(responses: Vec<(u16, String)>) -> (String, JoinHandle<Vec<Request>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let handle = thread::spawn(move || {
            let mut requests = Vec::new();
            for (status, body) in responses {
                let (stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream);
                let mut head = String::new();
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" || line.is_empty() {
                        break;
                    }
                    head.push_str(&line);
                }
                let length = head
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|value| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                let mut request_body = vec![0; length];
                reader.read_exact(&mut request_body).unwrap();
                let reply = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                reader.get_mut().write_all(reply.as_bytes()).unwrap();
                requests.push(Request {
                    head,
                    body: request_body,
                });
            }
            requests
        });
        (base, handle)
    }
}

#[cfg(test)]
mod tests {
    use super::{fake::serve, *};

    #[test]
    fn exchanges_a_github_token_for_a_project() {
        let (base, server) = serve(vec![
            (200, r#"{"count":1,"value":"github-jwt"}"#.into()),
            (
                200,
                r#"{"token":"ceagle_ABC","project":"gdash","expires_at":"2026-11-26T12:00:00Z"}"#
                    .into(),
            ),
        ]);
        let client = Client::new(&base);
        let github = client
            .github_token(
                &format!("{base}/token?api-version=2.0"),
                "request-secret",
                "ceagle.cc",
            )
            .unwrap();
        assert_eq!(github, "github-jwt");
        assert_eq!(client.exchange(&github, "gdash").unwrap(), "ceagle_ABC");
        let requests = server.join().unwrap();
        assert!(
            requests[0]
                .head
                .starts_with("GET /token?api-version=2.0&audience=ceagle.cc ")
        );
        assert!(
            requests[0]
                .head
                .to_lowercase()
                .contains("authorization: bearer request-secret")
        );
        assert!(
            requests[1]
                .head
                .starts_with("POST /api/v1/oidc/github/exchange ")
        );
        assert_eq!(
            requests[1].body,
            br#"{"token":"github-jwt","project":"gdash"}"#
        );
    }

    #[test]
    fn reads_problems_and_errors() {
        let (base, server) = serve(vec![
            (
                422,
                r#"{"error":"first","problems":["first","second"]}"#.into(),
            ),
            (
                403,
                r#"{"error":"no CEagle project trusts owner/repo"}"#.into(),
            ),
            (502, "<html>bad gateway</html>".into()),
        ]);
        let client = Client::new(&base);
        let upload = Upload {
            version: "1.0.0",
            notes: "",
            files: &[],
            dependencies: &BTreeMap::new(),
            attestation: None,
        };
        assert!(
            matches!(client.publish("gdash", "t", &upload), Err(ApiError::Problems(problems)) if problems.len() == 2)
        );
        assert!(matches!(
            client.exchange("jwt", "gdash"),
            Err(ApiError::Refused { status: 403, .. })
        ));
        assert!(matches!(
            client.release("gdash", "1.0.0", "t"),
            Err(ApiError::Refused { status: 502, .. })
        ));
        server.join().unwrap();
    }

    #[test]
    fn reports_an_unreachable_server_as_a_network_error() {
        let client = Client::new("http://127.0.0.1:9");
        assert!(matches!(
            client.release("gdash", "1.0.0", "t"),
            Err(ApiError::Network(_))
        ));
    }
}
