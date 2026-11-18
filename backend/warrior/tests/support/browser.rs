//! ChromeDriver と SafariDriver の起動、および WebDriver のセッションの作成。

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use thirtyfour::components::SelectElement;
use thirtyfour::prelude::*;
use thirtyfour::{ChromiumLikeCapabilities, LoggingPrefsLogLevel};

use super::DevServer;

const DRIVER_READY_TIMEOUT: Duration = Duration::from_secs(60);
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// E2E で使うブラウザ。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Browser {
    /// Chrome。
    Chrome,
    /// Safari。
    Safari,
}

impl Browser {
    /// 環境変数 `WARRIOR_E2E_BROWSER` から選ぶ。既定は Chrome。
    pub fn from_env() -> Self {
        match std::env::var("WARRIOR_E2E_BROWSER").as_deref() {
            Ok("safari") => Browser::Safari,
            _ => Browser::Chrome,
        }
    }

    fn driver_name(self) -> &'static str {
        match self {
            Browser::Chrome => "chromedriver",
            Browser::Safari => "safaridriver",
        }
    }
}

/// 起動した WebDriver。
pub struct DriverProcess {
    child: Child,
    port: u16,
}

impl DriverProcess {
    /// WebDriver を起動し、待ち受けを始めるまで待つ。
    pub fn start(browser: Browser, port: u16) -> Self {
        let mut command = Command::new(browser.driver_name());
        match browser {
            Browser::Chrome => {
                command.args([
                    format!("--port={port}"),
                    "--allowed-ips=127.0.0.1".to_owned(),
                ]);
            }
            Browser::Safari => {
                command.args(["--port", &port.to_string()]);
            }
        }
        command
            .process_group(0)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let child = command.spawn().unwrap_or_else(|error| {
            panic!(
                "{} must start (run this test with the matching mise task): {error}",
                browser.driver_name()
            )
        });
        let process = Self { child, port };
        wait_for_driver(process.port);
        process
    }

    /// WebDriver の接続先。
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
}

impl Drop for DriverProcess {
    fn drop(&mut self) {
        let group = -(self.child.id() as i32);
        let _ = Command::new("kill")
            .args(["-TERM", "--", &group.to_string()])
            .stderr(Stdio::null())
            .status();
        let _ = self.child.wait();
        let _ = Command::new("kill")
            .args(["-KILL", "--", &group.to_string()])
            .stderr(Stdio::null())
            .status();
    }
}

/// WebDriver のセッションを開く。
pub async fn open(browser: Browser, driver: &DriverProcess) -> WebDriver {
    let capabilities: thirtyfour::Capabilities = match browser {
        Browser::Chrome => chrome_capabilities().into(),
        Browser::Safari => DesiredCapabilities::safari().into(),
    };
    open_session(browser, driver, capabilities).await
}

/// 通知の許可とポップアップを許可した Chrome のセッションを開く。
///
/// 通知の許可はブラウザの設定の既定値で与える。ヘッドレスの Chrome には許可を求める
/// 画面が無いため、テストの間は通知を常に許可した状態にする。
pub async fn open_with_notification_permission(driver: &DriverProcess) -> WebDriver {
    let mut capabilities = chrome_capabilities();
    capabilities
        .add_experimental_option(
            "prefs",
            serde_json::json!({"profile.default_content_setting_values.notifications": 1}),
        )
        .expect("the notification preference must be set");
    capabilities
        .add_arg("--disable-popup-blocking")
        .expect("the popup blocker must be disabled");
    open_session(Browser::Chrome, driver, capabilities.into()).await
}

/// ダウンロード先を固定した Chrome のセッションを開く。
///
/// 書き出した CSV をブラウザに保存させ、テストがファイルとして読むために使う。
pub async fn open_with_download_dir(driver: &DriverProcess, directory: &Path) -> WebDriver {
    let mut capabilities = chrome_capabilities();
    capabilities
        .add_experimental_option(
            "prefs",
            serde_json::json!({
                "download.default_directory": directory.to_string_lossy(),
                "download.prompt_for_download": false,
                "download.directory_upgrade": true,
            }),
        )
        .expect("the download preference must be set");
    open_session(Browser::Chrome, driver, capabilities.into()).await
}

fn chrome_capabilities() -> thirtyfour::ChromeCapabilities {
    let mut capabilities = DesiredCapabilities::chrome();
    capabilities
        .set_browser_log_level(LoggingPrefsLogLevel::Info)
        .expect("browser logging");
    for argument in [
        "--headless=new",
        "--no-sandbox",
        "--disable-dev-shm-usage",
        "--disable-gpu",
        "--window-size=390,844",
        "--remote-allow-origins=*",
    ] {
        capabilities
            .add_arg(argument)
            .unwrap_or_else(|error| panic!("the chrome argument {argument} must be set: {error}"));
    }
    capabilities
}

async fn open_session(
    browser: Browser,
    driver: &DriverProcess,
    capabilities: thirtyfour::Capabilities,
) -> WebDriver {
    let session = WebDriver::new(driver.url(), capabilities)
        .await
        .unwrap_or_else(|error| panic!("the WebDriver session must open: {error}"));
    if browser == Browser::Chrome {
        session
            .set_window_rect(0, 0, 390, 844)
            .await
            .unwrap_or_else(|error| panic!("the window must be sized: {error}"));
    }
    session
}

/// テスト用の OIDC の提供者の利用者を選び、Google のログインを通す。
pub async fn login_with_google(driver: &WebDriver, server: &DevServer) {
    driver
        .goto(format!("{}/login", server.origin()))
        .await
        .expect("the login page must open");
    wait_for_route(driver, "/login").await;
    driver
        .find(By::XPath(
            "//button[contains(normalize-space(.), 'Google でログイン')]",
        ))
        .await
        .expect("the Google login button")
        .click()
        .await
        .expect("the Google login button must be clickable");
    wait_for_route(driver, "/").await;
}

/// ログアウトの要求を送る。
pub async fn logout(driver: &WebDriver) {
    driver
        .execute_async(
            "const done = arguments[arguments.length - 1]; fetch('/api/auth/logout', { method: 'POST' }).then((r) => r.text()).then((t) => done(t));",
            Vec::<serde_json::Value>::new(),
        )
        .await
        .expect("the logout must complete");
}

/// `data-route` が現れるまで待つ。
pub async fn wait_for_route(driver: &WebDriver, route: &str) {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let source = driver.source().await.unwrap_or_default();
        if source.contains(&format!("data-route=\"{route}\"")) {
            return;
        }
        if Instant::now() >= deadline {
            let logs = driver.get_log("browser").await.unwrap_or_default();
            let source = driver.source().await.unwrap_or_default();
            let preview = source.chars().take(2000).collect::<String>();
            panic!(
                "the route {route} did not appear; current URL: {:?}; logs: {logs:?}; source: {preview}",
                driver.current_url().await
            );
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// 画面の文字列が現れるまで待つ。
pub async fn wait_for_text(driver: &WebDriver, needle: &str) {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let source = driver.source().await.unwrap_or_default();
        if source.contains(needle) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the page did not contain {needle}: {source}"
        );
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// 文字列を持つボタンを押す。
pub async fn click_text(driver: &WebDriver, label: &str) {
    let button = driver
        .find(By::XPath(format!(
            "//button[contains(normalize-space(.), '{}')]",
            label
        )))
        .await
        .unwrap_or_else(|error| panic!("the button {label} must exist: {error}"));
    button.click().await.expect("the button must be clickable");
}

/// セレクタの要素を押す。
///
/// 前回の記録や自己ベストの読み込みで要素が動くと、押す位置が別の要素に当たる。
/// 要素を取り直して、押せるようになるまで繰り返す。
pub async fn click(driver: &WebDriver, selector: &str) {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        driver
            .execute(
                "document.querySelector(arguments[0]).scrollIntoView({block: 'center', inline: 'center'});",
                vec![serde_json::json!(selector)],
            )
            .await
            .ok();
        let element = driver
            .find(By::Css(selector))
            .await
            .unwrap_or_else(|error| panic!("the element {selector} must exist: {error}"));
        match element.click().await {
            Ok(()) => return,
            Err(error) => {
                assert!(
                    Instant::now() < deadline,
                    "the element {selector} must be clickable: {error}"
                );
                std::thread::sleep(POLL_INTERVAL);
            }
        }
    }
}

/// 数値の入力要素を空にして、値を入れる。
pub async fn set_input(driver: &WebDriver, selector: &str, value: &str) {
    let input = driver
        .find(By::Css(selector))
        .await
        .unwrap_or_else(|error| panic!("the input {selector} must exist: {error}"));
    input
        .clear()
        .await
        .unwrap_or_else(|error| panic!("the input {selector} must be cleared: {error}"));
    input
        .send_keys(value)
        .await
        .unwrap_or_else(|error| panic!("the input {selector} must be typed: {error}"));
}

/// セレクタの選択要素で、値を選ぶ。
pub async fn select_value(driver: &WebDriver, selector: &str, value: &str) {
    let select = driver
        .find(By::Css(selector))
        .await
        .unwrap_or_else(|error| panic!("the select {selector} must exist: {error}"));
    let select = SelectElement::new(&select)
        .await
        .unwrap_or_else(|error| panic!("the select {selector} must be a select: {error}"));
    select
        .select_by_value(value)
        .await
        .unwrap_or_else(|error| panic!("the value {value} must be selectable: {error}"));
}

/// 種目をライブラリから記録へ追加する。
pub async fn add_exercise(driver: &WebDriver, exercise_id: &str) {
    wait_for_present(driver, "[data-add-exercise]").await;
    click(driver, "[data-add-exercise]").await;
    wait_for_route(driver, "/record/:training_id/exercises").await;
    wait_for_present(driver, "[data-exercise-choose]").await;
    let selector = format!("[data-exercise-choose][data-exercise-id=\"{exercise_id}\"]");
    click(driver, &selector).await;
    wait_for_route(driver, "/record/:training_id").await;
    let row = format!("[data-exercise-row][data-exercise-id=\"{exercise_id}\"]");
    wait_for_present(driver, &row).await;
}

/// レールの種目を選ぶ。
pub async fn select_exercise(driver: &WebDriver, exercise_id: &str) {
    let row = format!("[data-exercise-row][data-exercise-id=\"{exercise_id}\"]");
    wait_for_present(driver, &row).await;
    click(driver, &row).await;
    wait_for_attribute(driver, &row, "data-selected", "true").await;
}

/// ページを開き、本文が現れるまで待って返す。
pub async fn get_text(driver: &WebDriver, url: &str) -> String {
    driver.goto(url).await.expect("the page must open");
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let text = driver
            .execute(
                "return document.body ? document.body.innerText : ''",
                Vec::<serde_json::Value>::new(),
            )
            .await
            .expect("the page text must be readable")
            .json()
            .as_str()
            .unwrap_or_default()
            .to_owned();
        if !text.is_empty() {
            return text;
        }
        assert!(
            Instant::now() < deadline,
            "the page text did not appear: {url}"
        );
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// 要素の属性を読む。
pub async fn attribute(driver: &WebDriver, selector: &str, attribute: &str) -> String {
    driver
        .execute(
            "const element = document.querySelector(arguments[0]); return element ? element.getAttribute(arguments[1]) : '';",
            vec![serde_json::json!(selector), serde_json::json!(attribute)],
        )
        .await
        .expect("the attribute must be readable")
        .json()
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

/// 入力要素の値を読む。
pub async fn value(driver: &WebDriver, selector: &str) -> String {
    driver
        .execute(
            "const element = document.querySelector(arguments[0]); return element ? element.value : '';",
            vec![serde_json::json!(selector)],
        )
        .await
        .expect("the value must be readable")
        .json()
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

/// 要素の属性が期待値になるまで待つ。
pub async fn wait_for_attribute(
    driver: &WebDriver,
    selector: &str,
    attribute_name: &str,
    expected: &str,
) {
    wait_for_attribute_within(
        driver,
        selector,
        attribute_name,
        expected,
        Duration::from_secs(60),
    )
    .await;
}

/// 要素の属性が、指定した時間の中で期待値になるまで待つ。
pub async fn wait_for_attribute_within(
    driver: &WebDriver,
    selector: &str,
    attribute_name: &str,
    expected: &str,
    timeout: Duration,
) {
    let deadline = Instant::now() + timeout;
    loop {
        if attribute(driver, selector, attribute_name).await == expected {
            return;
        }
        if Instant::now() >= deadline {
            let logs = driver.get_log("browser").await.unwrap_or_default();
            panic!(
                "the attribute {attribute_name} of {selector} did not become {expected} within {timeout:?}: URL {:?}; logs: {logs:?}",
                driver.current_url().await
            );
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// ページのコンソールに `panic:` を含むログが無いことを確かめる。
pub async fn assert_no_panic(driver: &WebDriver) {
    let logs = driver
        .get_log("browser")
        .await
        .expect("the browser logs must be readable");
    let panics = logs
        .iter()
        .filter(|entry| entry.message.contains("panic:"))
        .collect::<Vec<_>>();
    assert!(panics.is_empty(), "the page panicked: {panics:?}");
}

/// 通信の復帰をブラウザへ知らせる。
pub async fn online(driver: &WebDriver) {
    driver
        .execute(
            "window.dispatchEvent(new Event('online'));",
            Vec::<serde_json::Value>::new(),
        )
        .await
        .expect("the online event must be dispatched");
}

/// 要素が現れるまで待つ。
pub async fn wait_for_present(driver: &WebDriver, selector: &str) {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if driver.find(By::Css(selector)).await.is_ok() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the selector {selector} did not appear"
        );
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// 要素が消えるまで待つ。
pub async fn wait_for_absent(driver: &WebDriver, selector: &str) {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if driver.find(By::Css(selector)).await.is_err() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the selector {selector} did not disappear"
        );
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// 入力要素の値が期待値になるまで待つ。
pub async fn wait_for_value(driver: &WebDriver, selector: &str, expected: &str) {
    wait_for_value_within(driver, selector, expected, Duration::from_secs(60)).await;
}

/// 入力要素の値が、指定した時間の中で期待値になるまで待つ。
pub async fn wait_for_value_within(
    driver: &WebDriver,
    selector: &str,
    expected: &str,
    timeout: Duration,
) {
    let deadline = Instant::now() + timeout;
    loop {
        if value(driver, selector).await == expected {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the value of {selector} did not become {expected} within {timeout:?}"
        );
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// 非同期のスクリプトを実行し、JSON の結果を返す。
pub async fn eval_json(
    driver: &WebDriver,
    script: &str,
    arguments: Vec<serde_json::Value>,
) -> serde_json::Value {
    let result = driver
        .execute_async(script, arguments)
        .await
        .expect("the script must run");
    let text = result
        .json()
        .as_str()
        .expect("the script must return a JSON string")
        .to_owned();
    serde_json::from_str(&text).expect("valid JSON")
}

/// 非同期のスクリプトを実行し、文字列の結果を返す。
pub async fn eval_string(
    driver: &WebDriver,
    script: &str,
    arguments: Vec<serde_json::Value>,
) -> String {
    driver
        .execute_async(script, arguments)
        .await
        .expect("the script must run")
        .json()
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

/// 画面のセットの行の ID を並べる。
pub async fn set_ids(driver: &WebDriver) -> Vec<String> {
    let value = eval_json(
        driver,
        "const done = arguments[arguments.length - 1]; \
         done(JSON.stringify(Array.from(document.querySelectorAll('[data-set-row]')) \
           .map((element) => element.getAttribute('data-set-id'))));",
        Vec::new(),
    )
    .await;
    value
        .as_array()
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// 要素が有効になるまで待つ。
pub async fn wait_for_enabled(driver: &WebDriver, selector: &str) {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let enabled = eval_json(
            driver,
            "const done = arguments[arguments.length - 1]; \
             const element = document.querySelector(arguments[0]); \
             done(JSON.stringify(element ? !element.disabled : false));",
            vec![serde_json::json!(selector)],
        )
        .await
        .as_bool()
        .unwrap_or(false);
        if enabled {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the element {selector} did not become enabled"
        );
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// OS に空きポートを選ばせる。
pub fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a free port must be available");
    listener.local_addr().expect("a local address").port()
}

/// Chrome の CDP で通信を切る、または遅くする。
pub async fn set_network(driver: &WebDriver, offline: bool, latency: u64) {
    driver
        .cdp()
        .send_raw("Network.enable", serde_json::json!({}))
        .await
        .expect("the network must be enabled");
    driver
        .cdp()
        .send_raw(
            "Network.emulateNetworkConditions",
            serde_json::json!({
                "offline": offline,
                "latency": latency,
                "downloadThroughput": if offline { 0 } else { -1 },
                "uploadThroughput": if offline { 0 } else { -1 }
            }),
        )
        .await
        .expect("the network condition must change");
}

/// Chrome の CDP で通信を切る、または戻す。
pub async fn offline(driver: &WebDriver, offline: bool) {
    set_network(driver, offline, 0).await;
}

fn wait_for_driver(port: u16) {
    let deadline = Instant::now() + DRIVER_READY_TIMEOUT;
    loop {
        if driver_is_ready(port) {
            return;
        }
        if Instant::now() >= deadline {
            panic!("the WebDriver on port {port} did not become ready");
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

fn driver_is_ready(port: u16) -> bool {
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let Ok(mut stream) = TcpStream::connect_timeout(&address, Duration::from_millis(500)) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let request =
        format!("GET /status HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    if stream.write_all(request.as_bytes()).is_err() {
        return false;
    }
    let mut response = Vec::new();
    let mut buffer = [0u8; 1024];
    while let Ok(read) = stream.read(&mut buffer) {
        if read == 0 {
            break;
        }
        response.extend_from_slice(&buffer[..read]);
        if response.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    let text = String::from_utf8_lossy(&response);
    text.starts_with("HTTP/1.1 200") || text.starts_with("HTTP/1.0 200")
}
