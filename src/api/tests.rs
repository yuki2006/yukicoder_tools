//! HTTP まわりの挙動を、ローカルのモックサーバで確かめる。
//!
//! 本番の API を叩かずに、書き込みが PUT であること、ジャッジコードの API が
//! 無いサーバでも pull が止まらないことを固定する。

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::thread::JoinHandle;

use super::models::{ProblemEditRequest, ProblemSettings, Statement};
use super::YukicoderClient;

/// 受け取ったリクエストのメソッドを記録するだけのサーバ。
///
/// `put_fails` が true のときは PUT に 404 を返す。
fn serve(put_fails: bool) -> (String, JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    let handle = std::thread::spawn(move || {
        let mut seen = Vec::new();
        for stream in listener.incoming().take(1) {
            let mut stream = stream.unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());

            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();
            let method = request_line
                .split(' ')
                .next()
                .unwrap_or_default()
                .to_string();

            // ヘッダを読み飛ばしつつ、本文の長さだけ拾う。
            let mut length = 0usize;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line.trim().is_empty() {
                    break;
                }
                let lower = line.to_ascii_lowercase();
                if let Some(value) = lower.strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0u8; length];
            reader.read_exact(&mut body).unwrap();

            let (status, payload) = if method == "PUT" && put_fails {
                (404, "Not Found")
            } else {
                (200, r#"{"Message":"保存されました。"}"#)
            };
            write!(
                stream,
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                payload.len()
            )
            .unwrap();
            stream.flush().unwrap();

            seen.push(method);
        }
        seen
    });

    (format!("http://{addr}"), handle)
}

/// 1 リクエストだけ受けて、決めた応答を返すサーバ。
fn serve_once(status: u16, payload: &'static str) -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    let handle = std::thread::spawn(move || {
        let mut stream = listener.incoming().next().unwrap().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if line.trim().is_empty() {
                break;
            }
        }
        write!(
            stream,
            "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{payload}",
            payload.len()
        )
        .unwrap();
        stream.flush().unwrap();
    });

    (format!("http://{addr}"), handle)
}

#[test]
fn judge_code_is_parsed() {
    let (base_url, server) = serve_once(200, r#"{"langId":"cpp17","source":"x","status":"AC"}"#);
    let client = YukicoderClient::new("dummy-token".into(), base_url).unwrap();

    let code = client.get_judge_code(13954).unwrap();

    assert_eq!(code.lang_id, "cpp17");
    assert_eq!(code.source, "x");
    assert_eq!(code.status, "AC");
    server.join().unwrap();
}

/// validator のレスポンスを読める。使わないフィールド (latestCheck /
/// testCaseLatest) が付いていても壊れない。行が無いときは全フィールドが空。
#[test]
fn validator_is_parsed() {
    let (base_url, server) = serve_once(
        200,
        r#"{"langId":"cpp17","source":"x","status":"AC",
           "latestCheck":1756900000000000000,"testCaseLatest":1756800000000000000}"#,
    );
    let client = YukicoderClient::new("dummy-token".into(), base_url).unwrap();

    let validator = client.get_validator(13954).unwrap();

    assert_eq!(validator.lang_id, "cpp17");
    assert_eq!(validator.status, "AC");
    let judging = std::collections::HashSet::from(["WJ".to_string()]);
    assert!(validator.is_up_to_date(&judging));
    server.join().unwrap();

    let (base_url, server) = serve_once(200, r#"{"langId":"","source":"","status":""}"#);
    let client = YukicoderClient::new("dummy-token".into(), base_url).unwrap();
    let empty = client.get_validator(13954).unwrap();
    assert!(!empty.is_up_to_date(&judging), "未登録は未完了");
    server.join().unwrap();
}

#[test]
fn statuses_are_parsed() {
    let (base_url, server) = serve_once(
        200,
        r#"[{"id":"WJ","category":"judging","description":"待ち"},
           {"id":"AC","category":"success","description":"正解"}]"#,
    );
    let client = YukicoderClient::new("dummy-token".into(), base_url).unwrap();
    let statuses = client.statuses().unwrap();
    assert_eq!(statuses.len(), 2);
    let judging = super::models::judging_ids(&statuses);
    assert!(judging.contains("WJ") && !judging.contains("AC"));
    server.join().unwrap();
}

/// テストケース一覧は `?detail=1` のハッシュ付きの形で読み、名前も検証する。
#[test]
fn testcase_listing_is_parsed_with_hashes() {
    use super::models::Which;

    let (base_url, server) = serve_once(
        200,
        r#"[{"name":"1.txt","size":4,"sha256":"9f64a747e1b97f131fabb6b447296c9b6f0201e79fb3c5356e6c77e89b6a806a"}]"#,
    );
    let client = YukicoderClient::new("dummy-token".into(), base_url).unwrap();
    let details = client.list_testcases_detail(13954, Which::In).unwrap();
    assert_eq!(details[0].name, "1.txt");
    assert!(details[0].sha256.starts_with("9f64a747"));
    server.join().unwrap();

    let (base_url, server) = serve_once(200, r#"[{"name":"../evil","size":1,"sha256":"ab"}]"#);
    let client = YukicoderClient::new("dummy-token".into(), base_url).unwrap();
    assert!(
        client.list_testcases_detail(13954, Which::In).is_err(),
        "サーバ由来の名前も検証する"
    );
    server.join().unwrap();
}

/// テストケース名の規則 (使える文字の一覧) はサーバから取得する。
#[test]
fn testcase_name_rule_is_fetched() {
    let (base_url, server) = serve_once(200, r#"{"allowedChars":"abc-"}"#);
    let client = YukicoderClient::new("dummy-token".into(), base_url).unwrap();
    assert_eq!(client.testcase_name_rule().unwrap(), "abc-");
    server.join().unwrap();
}

/// サーバ由来のテストケース名も検証する。ローカルパスに join し URL にも
/// 埋め込むので、'/' や '..' を通すと外に書いてしまう。これは命名規則の
/// 写しではなく、クライアント自身の安全条件。
#[test]
fn testcase_names_from_the_server_are_validated() {
    for (payload, ok) in [
        (r#"["1.txt","sample_1.txt","case-01.txt"]"#, true),
        (r#"["../evil"]"#, false),
        (r#"["a/b.txt"]"#, false),
        (r#"["c:evil"]"#, false),
        (r#"[""]"#, false),
    ] {
        let (base_url, server) = serve_once(200, payload);
        let client = YukicoderClient::new("dummy-token".into(), base_url).unwrap();
        let result = client.list_testcases(13954, super::models::Which::In);
        assert_eq!(result.is_ok(), ok, "{payload}");
        server.join().unwrap();
    }
}

fn request() -> ProblemEditRequest {
    let settings = ProblemSettings {
        title: "タイトル".into(),
        tags: String::new(),
        level: 1.0,
        time_limit_ms: 2000,
        memory_limit: 1024,
        eps_mode: "-".into(),
        eps: "0.0".into(),
        wip: true,
        recruiting_tester: false,
        problem_type: 0,
        judge_type: 0,
        show_ans: true,
        enable_pure_judge: false,
        force_single_server_judge: false,
        allowed_langs: vec![],
    };
    ProblemEditRequest::new(settings, Statement::Markdown("本文".into())).unwrap()
}

#[test]
fn uses_put_for_writes() {
    let (base_url, server) = serve(false);
    let client = YukicoderClient::new("dummy-token".into(), base_url).unwrap();

    let res = client.save_problem_edit(13954, &request()).unwrap();

    assert_eq!(res.message, "保存されました。");
    assert_eq!(server.join().unwrap(), vec!["PUT"]);
}

/// PUT が通らないときに POST で送り直したりしないこと。
///
/// 黙って別のメソッドで送り直すと、原因の分からない失敗になる。
#[test]
fn does_not_retry_with_another_method() {
    let (base_url, server) = serve(true);
    let client = YukicoderClient::new("dummy-token".into(), base_url).unwrap();

    let err = client.save_problem_edit(13954, &request()).unwrap_err();

    assert!(err.to_string().contains("404"), "{err}");
    assert_eq!(server.join().unwrap(), vec!["PUT"]);
}

// ---- CLI ログインと AI生成コード置き場 -------------------------------------

/// サーバが受け取ったリクエスト。
pub(crate) struct Recorded {
    pub method: String,
    pub path: String,
    pub authorization: Option<String>,
    pub content_type: Option<String>,
    pub body: String,
}

/// 決めた応答を順に返し、受け取ったリクエストを記録するサーバ。
///
/// 応答の数だけ接続を受けたら終わる (毎回 `Connection: close` で切る)。
pub(crate) fn serve_sequence(
    responses: Vec<(u16, &'static str)>,
) -> (String, JoinHandle<Vec<Recorded>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    let handle = std::thread::spawn(move || {
        let mut seen = Vec::new();
        for (status, payload) in responses {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());

            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();
            let mut parts = request_line.split(' ');
            let method = parts.next().unwrap_or_default().to_string();
            let path = parts.next().unwrap_or_default().to_string();

            let mut length = 0usize;
            let mut authorization = None;
            let mut content_type = None;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line.trim().is_empty() {
                    break;
                }
                let Some((name, value)) = line.split_once(':') else {
                    continue;
                };
                let value = value.trim().to_string();
                match name.to_ascii_lowercase().as_str() {
                    "content-length" => length = value.parse().unwrap_or(0),
                    "authorization" => authorization = Some(value),
                    "content-type" => content_type = Some(value),
                    _ => {}
                }
            }
            let mut body = vec![0u8; length];
            reader.read_exact(&mut body).unwrap();

            write!(
                stream,
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                payload.len()
            )
            .unwrap();
            stream.flush().unwrap();

            seen.push(Recorded {
                method,
                path,
                authorization,
                content_type,
                body: String::from_utf8(body).unwrap(),
            });
        }
        seen
    });

    (format!("http://{addr}"), handle)
}

const DEVICE_CODE_RESPONSE: &str = r#"{"device_code":"ydc_secret","user_code":"WDJB-MJHT",
    "verification_uri":"https://yukicoder.me/device",
    "verification_uri_complete":"https://yukicoder.me/device?user_code=WDJB-MJHT",
    "expires_in":600,"interval":5}"#;

/// scope は求めたときだけ送る (要らない権限を求めない)。認証は付けない。
#[test]
fn device_code_sends_scope_only_when_asked() {
    let (base_url, server) = serve_sequence(vec![
        (200, DEVICE_CODE_RESPONSE),
        (200, DEVICE_CODE_RESPONSE),
    ]);
    let client = YukicoderClient::anonymous(base_url).unwrap();

    let Ok(code) = client.device_code("yuki-tool", None) else {
        panic!("device_code に失敗した");
    };
    assert_eq!(code.user_code, "WDJB-MJHT");
    assert_eq!(code.device_code, "ydc_secret");
    assert_eq!(code.expires_in, 600);
    assert_eq!(code.interval, 5);
    assert!(client.device_code("yuki-tool", Some("ai_code")).is_ok());

    let requests = server.join().unwrap();
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].path, "/v1/device/code");
    assert_eq!(requests[0].authorization, None);
    assert_eq!(
        requests[0].content_type.as_deref(),
        Some("application/x-www-form-urlencoded")
    );
    assert_eq!(requests[0].body, "client_name=yuki-tool");
    assert_eq!(requests[1].body, "client_name=yuki-tool&scope=ai_code");
}

/// 知らない scope は 400 invalid_scope。理由をそのまま見せる。
#[test]
fn device_code_reports_invalid_scope() {
    let (base_url, server) = serve_sequence(vec![(
        400,
        r#"{"error":"invalid_scope","error_description":"scope x は使えません"}"#,
    )]);
    let client = YukicoderClient::anonymous(base_url).unwrap();

    let err = match client.device_code("yuki-tool", Some("x")) {
        Ok(_) => panic!("invalid_scope なのに成功した"),
        Err(err) => format!("{err:#}"),
    };

    assert!(err.contains("invalid_scope"), "{err}");
    server.join().unwrap();
}

/// 解釈できないレスポンスでも、本文 (device_code やトークン) をエラーに出さない。
#[test]
fn device_responses_are_not_echoed_on_parse_errors() {
    let (base_url, server) = serve_sequence(vec![
        (200, r#"{"device_code":"ydc_secret"}"#),
        (200, r#"{"access_token":"ysb_secret"}"#),
    ]);
    let client = YukicoderClient::anonymous(base_url).unwrap();

    let code_err = match client.device_code("yuki-tool", None) {
        Ok(_) => panic!("欠けたレスポンスを読めてしまった"),
        Err(err) => format!("{err:#}"),
    };
    let token_err = match client.device_token("ydc_secret") {
        Ok(_) => panic!("欠けたレスポンスを読めてしまった"),
        Err(err) => format!("{err:#}"),
    };

    assert!(!code_err.contains("ydc_secret"), "{code_err}");
    assert!(!token_err.contains("ysb_secret"), "{token_err}");
    assert!(!token_err.contains("ydc_secret"), "{token_err}");
    server.join().unwrap();
}

const AI_CODE_ITEM: &str = r#"{"id":7,"userId":10,"title":"segtree","lang":"C++23","size":3,
    "updatedAt":"2026-11-15T12:00:00+09:00","url":"/users/10/ai_code/7",
    "rawUrl":"/users/10/ai_code/7/raw"}"#;

/// 投稿は POST、差し替えは PUT、タイトル・言語だけの変更は PATCH。
/// どれも JSON で、Bearer を付ける。PATCH は指定したものだけを送る
/// (未知のフィールドはサーバがエラーにする)。
#[test]
fn ai_code_writes_use_the_documented_methods() {
    use super::ai_code::{AiCodeInput, AiCodeLabel};

    let (base_url, server) = serve_sequence(vec![
        (201, AI_CODE_ITEM),
        (200, AI_CODE_ITEM),
        (200, AI_CODE_ITEM),
    ]);
    let client = YukicoderClient::new("dummy-token".into(), base_url).unwrap();
    let input = AiCodeInput {
        title: "segtree".into(),
        lang: "C++23".into(),
        source: "abc".into(),
    };

    let created = client.create_ai_code(&input).unwrap();
    assert_eq!(created.id, 7);
    assert_eq!(created.user_id, 10);
    assert_eq!(created.updated_at, "2026-11-15T12:00:00+09:00");
    assert!(created.source.is_none());
    client.update_ai_code(7, &input).unwrap();
    client
        .label_ai_code(
            7,
            &AiCodeLabel {
                title: None,
                lang: Some("Rust".into()),
            },
        )
        .unwrap();

    let requests = server.join().unwrap();
    let seen: Vec<(&str, &str)> = requests
        .iter()
        .map(|r| (r.method.as_str(), r.path.as_str()))
        .collect();
    assert_eq!(
        seen,
        [
            ("POST", "/v1/ai_code"),
            ("PUT", "/v1/ai_code/7"),
            ("PATCH", "/v1/ai_code/7")
        ]
    );
    for request in &requests {
        assert_eq!(request.authorization.as_deref(), Some("Bearer dummy-token"));
        assert_eq!(request.content_type.as_deref(), Some("application/json"));
    }
    let body = r#"{"title":"segtree","lang":"C++23","source":"abc"}"#;
    assert_eq!(requests[0].body, body);
    assert_eq!(requests[1].body, body);
    assert_eq!(requests[2].body, r#"{"lang":"Rust"}"#);
}

/// 読むのは認証不要なので、トークンを持っていても付けない。
#[test]
fn ai_code_reads_do_not_send_the_token() {
    let (base_url, server) = serve_sequence(vec![
        (200, "[]"),
        (
            200,
            r#"{"id":7,"userId":10,"title":"t","lang":"l","size":3,
               "updatedAt":"2026-11-15T12:00:00+09:00","source":"abc"}"#,
        ),
        (200, "[]"),
    ]);
    let client = YukicoderClient::new("dummy-token".into(), base_url).unwrap();

    assert!(client.user_ai_codes(10).unwrap().is_empty());
    assert_eq!(
        client.user_ai_code(10, 7).unwrap().source.as_deref(),
        Some("abc")
    );
    // 自分の一覧は認証が要る。
    assert!(client.my_ai_codes().unwrap().is_empty());

    let requests = server.join().unwrap();
    assert_eq!(requests[0].path, "/v1/users/10/ai_code");
    assert_eq!(requests[0].authorization, None);
    assert_eq!(requests[1].path, "/v1/users/10/ai_code/7");
    assert_eq!(requests[1].authorization, None);
    assert_eq!(requests[2].path, "/v1/ai_code");
    assert_eq!(
        requests[2].authorization.as_deref(),
        Some("Bearer dummy-token")
    );
}

/// 401 は型で見分けられる (保存したログインを消してやり直す判断に使う)。
/// メッセージにはサーバの案内が入る。
#[test]
fn unauthorized_is_detectable() {
    let (base_url, server) = serve_sequence(vec![
        (
            401,
            r#"{"error":"scope=ai_code を付けて受け取り直してください"}"#,
        ),
        (404, r#"{"error":"コードが見つかりません"}"#),
    ]);
    let client = YukicoderClient::new("dummy-token".into(), base_url).unwrap();

    let unauthorized = client.my_ai_codes().unwrap_err();
    let not_found = client.my_ai_codes().unwrap_err();

    assert!(super::is_unauthorized(&unauthorized));
    assert!(format!("{unauthorized:#}").contains("scope=ai_code"));
    assert!(!super::is_unauthorized(&not_found));
    // with_context で包んでも見分けられる。
    assert!(super::is_unauthorized(&unauthorized.context("包んだ")));
    server.join().unwrap();
}
