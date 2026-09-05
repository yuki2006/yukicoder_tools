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
