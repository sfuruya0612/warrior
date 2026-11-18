#![allow(dead_code)]

//! `wrangler dev` とテスト専用の OIDC の提供者を起動する E2E の支援。
//!
//! 待機は health の応答を条件にし、固定の sleep で起動を待たない。
//! 子プロセスはプロセスグループごと停止し、テストが作った資源は teardown で削除する。

use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub mod browser;
pub mod ownership;
pub mod record_browser;
pub mod record_common;
pub mod record_unilateral_superset;

const STARTUP_TIMEOUT: Duration = Duration::from_secs(90);
const POLL_INTERVAL: Duration = Duration::from_millis(100);
const R2_BUCKET: &str = "warrior-local";
static R2_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// 起動した `wrangler dev`。
pub struct DevServer {
    child: Child,
    port: u16,
    persist_dir: PathBuf,
    log_path: PathBuf,
}

impl DevServer {
    /// `wrangler dev` をローカルモードで起動し、health が 200 になるまで待つ。
    pub fn start(port: u16) -> Self {
        Self::start_with_vars(port, &[])
    }

    /// vars を上書きして `wrangler dev` を起動する。
    pub fn start_with_vars(port: u16, vars: &[(&str, &str)]) -> Self {
        wait_for_port_free(port);
        let persist_dir = unique_persist_dir();
        fs::create_dir_all(&persist_dir).expect("the persistence directory must be created");
        let root = repository_root();
        let log_path = persist_dir.join("wrangler.log");
        let log_file = fs::File::create(&log_path).expect("the log file must be created");
        let log_error = log_file.try_clone().expect("the log file must be cloned");
        let mut command = Command::new("wrangler");
        command.args([
            "dev",
            "--local",
            "--ip",
            "127.0.0.1",
            "--port",
            &port.to_string(),
            "--persist-to",
            persist_dir.to_str().expect("a UTF-8 path"),
        ]);
        for (name, value) in vars {
            command.args(["--var", &format!("{name}:{value}")]);
        }
        let child = command
            .current_dir(root)
            .process_group(0)
            .stdout(Stdio::from(log_file))
            .stderr(Stdio::from(log_error))
            .spawn()
            .expect("wrangler dev must start");
        let server = Self {
            child,
            port,
            persist_dir,
            log_path,
        };
        server.wait_until_healthy();
        server
    }

    /// サーバーのオリジン。
    pub fn origin(&self) -> String {
        format!("http://localhost:{}", self.port)
    }

    /// ヘッダを指定してリクエストを送る。
    pub fn request(&self, method: &str, path: &str, headers: &[(&str, &str)]) -> HttpResponse {
        http_request(self.port, method, path, headers, None).expect("HTTP request")
    }

    /// 本文を指定してリクエストを送る。
    pub fn request_with_body(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: &str,
    ) -> HttpResponse {
        http_request(self.port, method, path, headers, Some(body)).expect("HTTP request")
    }

    /// `wrangler dev` の出力を読む。
    pub fn log_lines(&self) -> Vec<String> {
        fs::read_to_string(&self.log_path)
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// 出力に指定した文字列が現れるまで待つ。
    pub fn wait_for_log(&self, needle: &str) -> Vec<String> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let lines = self.log_lines();
            if lines.iter().any(|line| line.contains(needle)) {
                return lines;
            }
            if Instant::now() >= deadline {
                panic!("the log did not contain {needle} within 10 seconds: {lines:?}");
            }
            std::thread::sleep(POLL_INTERVAL);
        }
    }

    /// マイグレーションをローカルの D1 に適用する。
    pub fn apply_migrations(&self) -> Result<(), String> {
        self.run_wrangler(&[
            "d1",
            "migrations",
            "apply",
            "DB",
            "--local",
            "--persist-to",
            self.persist_dir_str(),
        ])
        .map(|_| ())
    }

    /// ローカルの D1 で SQL を実行し、結果の JSON を返す。
    pub fn execute_d1_json(&self, sql: &str) -> Result<serde_json::Value, String> {
        let output = self.run_wrangler(&[
            "d1",
            "execute",
            "DB",
            "--local",
            "--persist-to",
            self.persist_dir_str(),
            "--command",
            sql,
            "--json",
        ])?;
        serde_json::from_str(&output)
            .map_err(|error| format!("the d1 output must be JSON but was {output}: {error}"))
    }

    /// ローカルの D1 で SQL ファイルを実行する。
    pub fn execute_d1_file(&self, path: &Path) -> Result<(), String> {
        self.run_wrangler(&[
            "d1",
            "execute",
            "DB",
            "--local",
            "--persist-to",
            self.persist_dir_str(),
            "--file",
            path.to_str().expect("a UTF-8 path"),
            "--json",
        ])
        .map(|_| ())
    }

    /// ローカルの R2 にオブジェクトを置く。
    pub fn put_r2_object(&self, key: &str, bytes: &[u8]) -> Result<(), String> {
        let sequence = R2_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = self
            .persist_dir
            .join(format!("r2-object-{}-{sequence}.bin", std::process::id()));
        fs::write(&path, bytes)
            .map_err(|error| format!("failed to write {}: {error}", path.display()))?;
        let object = format!("{R2_BUCKET}/{key}");
        let result = self.run_wrangler(&[
            "r2",
            "object",
            "put",
            &object,
            "--local",
            "--persist-to",
            self.persist_dir_str(),
            "--file",
            path.to_str().expect("a UTF-8 path"),
            "--content-type",
            "application/octet-stream",
            "--force",
        ]);
        let _ = fs::remove_file(&path);
        result.map(|_| ())
    }

    /// ローカルの R2 からオブジェクトを読む。無いときは None。
    pub fn get_r2_object(&self, key: &str) -> Result<Option<Vec<u8>>, String> {
        let sequence = R2_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = self
            .persist_dir
            .join(format!("r2-object-{}-{sequence}.bin", std::process::id()));
        let object = format!("{R2_BUCKET}/{key}");
        let output = self.run_wrangler_raw(&[
            "r2",
            "object",
            "get",
            &object,
            "--local",
            "--persist-to",
            self.persist_dir_str(),
            "--file",
            path.to_str().expect("a UTF-8 path"),
        ]);
        match output {
            Ok(_) => {
                let bytes = fs::read(&path)
                    .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
                let _ = fs::remove_file(&path);
                Ok(Some(bytes))
            }
            Err(error) if error.contains("The specified key does not exist.") => {
                let _ = fs::remove_file(&path);
                Ok(None)
            }
            Err(error) => {
                let _ = fs::remove_file(&path);
                Err(error)
            }
        }
    }

    /// ローカルの R2 からオブジェクトを削除する。
    pub fn delete_r2_object(&self, key: &str) -> Result<(), String> {
        let object = format!("{R2_BUCKET}/{key}");
        self.run_wrangler(&[
            "r2",
            "object",
            "delete",
            &object,
            "--local",
            "--persist-to",
            self.persist_dir_str(),
            "--force",
        ])
        .map(|_| ())
    }

    fn wait_until_healthy(&self) {
        if wait_for_http_until(self.port, "/api/health", STARTUP_TIMEOUT) {
            return;
        }
        let lines = self.log_lines();
        let start = lines.len().saturating_sub(40);
        let tail = lines[start..].join("\n");
        panic!(
            "the server on port {} did not become ready within {STARTUP_TIMEOUT:?}:\n{tail}",
            self.port
        );
    }

    fn connect(&self, timeout: Duration) -> std::io::Result<TcpStream> {
        let address = SocketAddr::from(([127, 0, 0, 1], self.port));
        let stream = TcpStream::connect_timeout(&address, timeout)?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        Ok(stream)
    }

    fn persist_dir_str(&self) -> &str {
        self.persist_dir.to_str().expect("a UTF-8 path")
    }

    fn run_wrangler(&self, arguments: &[&str]) -> Result<String, String> {
        self.run_wrangler_raw(arguments)
    }

    fn run_wrangler_raw(&self, arguments: &[&str]) -> Result<String, String> {
        let output = Command::new("wrangler")
            .args(arguments)
            .current_dir(repository_root())
            .stdin(Stdio::null())
            .output()
            .map_err(|error| format!("failed to run wrangler: {error}"))?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            Err(format!(
                "wrangler {:?} failed with {}\nstdout:\n{}\nstderr:\n{}",
                arguments,
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ))
        }
    }
}

impl Drop for DevServer {
    fn drop(&mut self) {
        stop_process_group(&mut self.child);
        let _ = fs::remove_dir_all(&self.persist_dir);
    }
}

/// 起動したテスト専用の OIDC の提供者。
pub struct TestOidc {
    child: Child,
    port: u16,
}

impl TestOidc {
    /// テスト専用の OIDC の提供者を起動し、health が 200 になるまで待つ。
    pub fn start(port: u16) -> Self {
        wait_for_port_free(port);
        let binary =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/warrior_test_oidc");
        let child = Command::new(&binary)
            .args(["--port", &port.to_string()])
            .process_group(0)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap_or_else(|error| {
                panic!(
                    "the test OIDC provider must start from {} (build it with mise run test-oidc:build): {error}",
                    binary.display()
                )
            });
        let provider = Self { child, port };
        wait_for_http(port, "/health");
        provider
    }

    /// 提供者のオリジン。
    pub fn origin(&self) -> String {
        format!("http://localhost:{}", self.port)
    }

    /// 提供者へリクエストを送る。
    pub fn request(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: Option<&str>,
    ) -> HttpResponse {
        http_request(self.port, method, path, headers, body).expect("HTTP request")
    }
}

impl Drop for TestOidc {
    fn drop(&mut self) {
        stop_process_group(&mut self.child);
    }
}

fn stop_process_group(child: &mut Child) {
    let group = -(child.id() as i32);
    // wrangler は workerd を、safaridriver は Safari を別のプロセスとして起動する。
    let _ = Command::new("kill")
        .args(["-TERM", "--", &group.to_string()])
        .stderr(Stdio::null())
        .status();
    let _ = child.wait();
    let _ = Command::new("kill")
        .args(["-KILL", "--", &group.to_string()])
        .stderr(Stdio::null())
        .status();
}

fn wait_for_port_free(port: u16) {
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    while TcpStream::connect_timeout(&address, Duration::from_millis(100)).is_ok() {
        assert!(
            Instant::now() < deadline,
            "the port {port} is still used by another process"
        );
        std::thread::sleep(POLL_INTERVAL);
    }
}

fn wait_for_http(port: u16, path: &str) {
    if wait_for_http_until(port, path, STARTUP_TIMEOUT) {
        return;
    }
    panic!("the server on port {port} did not become ready within {STARTUP_TIMEOUT:?}");
}

fn wait_for_http_until(port: u16, path: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if let Ok(response) = try_http(port, path) {
            if response.status == 200 {
                return true;
            }
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

fn try_http(port: u16, path: &str) -> std::io::Result<HttpResponse> {
    http_request(port, "GET", path, &[], None)
}

fn http_request(
    port: u16,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&str>,
) -> std::io::Result<HttpResponse> {
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(5))?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    send_request(&mut stream, port, method, path, headers, body)?;
    Ok(HttpResponse::parse(&read_response(&mut stream)))
}

/// HTTP の応答。
#[derive(Debug)]
pub struct HttpResponse {
    /// 状態コード。
    pub status: u16,
    /// ヘッダ。名前は小文字にする。
    pub headers: Vec<(String, String)>,
    /// 本文。
    pub body: String,
}

impl HttpResponse {
    /// ヘッダの値を小文字の名前で引く。
    pub fn header(&self, name: &str) -> Option<&str> {
        let name = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(candidate, _)| *candidate == name)
            .map(|(_, value)| value.as_str())
    }

    fn parse(bytes: &[u8]) -> Self {
        let split = header_end(bytes).expect("a response must have a header terminator");
        let header_text = String::from_utf8_lossy(&bytes[..split]);
        let mut lines = header_text.split("\r\n");
        let status_line = lines.next().expect("a status line");
        let status = status_line
            .split_whitespace()
            .nth(1)
            .and_then(|value| value.parse().ok())
            .expect("a status code");
        let headers = lines
            .filter_map(|line| {
                let (name, value) = line.split_once(':')?;
                Some((name.trim().to_ascii_lowercase(), value.trim().to_owned()))
            })
            .collect::<Vec<(String, String)>>();
        let mut body = bytes[split + 4..].to_vec();
        if headers
            .iter()
            .any(|(name, value)| name == "transfer-encoding" && value.contains("chunked"))
        {
            body = decode_chunked(&body);
        }

        Self {
            status,
            headers,
            body: String::from_utf8_lossy(&body).into_owned(),
        }
    }
}

fn send_request(
    stream: &mut TcpStream,
    port: u16,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&str>,
) -> std::io::Result<()> {
    let body = body.unwrap_or("");
    let mut request = format!(
        "{method} {path} HTTP/1.0\r\nHost: 127.0.0.1:{port}\r\nContent-Length: {}\r\n",
        body.len()
    );
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str("\r\n");
    request.push_str(body);
    stream.write_all(request.as_bytes())
}

fn read_response(stream: &mut TcpStream) -> Vec<u8> {
    let mut response = Vec::new();
    let mut buffer = [0u8; 4096];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => {
                response.extend_from_slice(&buffer[..read]);
                if response_is_complete(&response) {
                    break;
                }
            }
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                break;
            }
            Err(error) => panic!("failed to read the response: {error}"),
        }
    }
    response
}

fn response_is_complete(response: &[u8]) -> bool {
    let Some(split) = header_end(response) else {
        return false;
    };
    let header_text = String::from_utf8_lossy(&response[..split]).to_ascii_lowercase();
    if header_text.contains("transfer-encoding: chunked") {
        return chunked_complete(&response[split + 4..]);
    }
    if let Some(length) = content_length(&header_text) {
        return response.len() >= split + 4 + length;
    }
    false
}

fn header_end(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|window| window == b"\r\n\r\n")
}

fn content_length(header_text: &str) -> Option<usize> {
    header_text.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if name.trim() == "content-length" {
            value.trim().parse().ok()
        } else {
            None
        }
    })
}

fn chunked_complete(body: &[u8]) -> bool {
    let mut position = 0;
    loop {
        let Some(line_end) = body[position..]
            .windows(2)
            .position(|window| window == b"\r\n")
        else {
            return false;
        };
        let size_text = String::from_utf8_lossy(&body[position..position + line_end]);
        let Ok(size) = usize::from_str_radix(size_text.trim(), 16) else {
            return false;
        };
        position += line_end + 2;
        if size == 0 {
            return body.len() >= position + 2;
        }
        position += size + 2;
        if position > body.len() {
            return false;
        }
    }
}

fn decode_chunked(body: &[u8]) -> Vec<u8> {
    let mut decoded = Vec::new();
    let mut position = 0;
    loop {
        let Some(line_end) = body[position..]
            .windows(2)
            .position(|window| window == b"\r\n")
        else {
            return decoded;
        };
        let size_text = String::from_utf8_lossy(&body[position..position + line_end]);
        let Ok(size) = usize::from_str_radix(size_text.trim(), 16) else {
            return decoded;
        };
        position += line_end + 2;
        if size == 0 {
            return decoded;
        }
        decoded.extend_from_slice(&body[position..position + size]);
        position += size + 2;
    }
}

fn unique_persist_dir() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!("warrior-e2e-{}-{nanos}", std::process::id()))
}

fn repository_root() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../..")
}

/// E2E で使う一意な利用者 ID。
pub fn unique_user_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let hex = format!("{nanos:032x}");
    let mut digits = hex[..32].chars().collect::<Vec<char>>();
    digits[12] = '4';
    digits[16] = '8';
    let text = digits.into_iter().collect::<String>();
    format!(
        "{}-{}-{}-{}-{}",
        &text[0..8],
        &text[8..12],
        &text[12..16],
        &text[16..20],
        &text[20..32]
    )
}

/// ログイン中の利用者の ID を返す。
pub async fn current_user_id(driver: &thirtyfour::WebDriver) -> String {
    let me = browser::eval_string(
        driver,
        "const done = arguments[arguments.length - 1]; fetch('/api/me').then((response) => response.text()).then((text) => done(text));",
        Vec::new(),
    )
    .await;
    serde_json::from_str::<serde_json::Value>(&me)
        .expect("the me response is JSON")
        .get("id")
        .and_then(|value| value.as_str())
        .expect("the user id")
        .to_owned()
}

/// 記録に使う体重を身体情報へ入れる。
pub fn seed_profile(server: &DevServer, user_id: &str) {
    server
        .execute_d1_json(&format!(
            "INSERT INTO profiles (user_id, weight_kg, created_at, updated_at) \
             VALUES ('{user_id}', 70.0, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')"
        ))
        .expect("the profile must be seeded");
}
