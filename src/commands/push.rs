//! `yuki-tool push` — ローカルのファイルを yukicoder に反映する。
//!
//! 書き込みは PUT (全置換)。部分更新ではないので問題設定は毎回すべて送る。問題文が空のまま
//! 送ると保存済みの問題文が消えるため、`ProblemEditRequest::new` で止める。

use std::time::{Duration, Instant};

use anyhow::{bail, Context as _, Result};

use crate::api::models::{
    judging_ids, EditorialRequest, GeneratorRequest, JudgeCodeRequest, ProblemEditRequest,
    SaveResponse, ValidatorRequest, Which, JUDGE_STATUS_OK,
};
use crate::api::YukicoderClient;
use crate::commands::Context;
use crate::local::{check_testcase_name, display_path, sha256_hex, ProblemDir};
use crate::Target;

#[derive(Debug, Clone, Copy)]
pub struct Options {
    pub dry_run: bool,
    pub testcases: bool,
    /// ローカルに無いテストケースをサーバから消す。
    pub prune: bool,
    /// ジェネレータ保存後にケース生成を起動する。
    pub generate: bool,
    /// ジャッジコードのコンパイル結果と validator の検証結果を待つ。
    pub wait_compile: bool,
}

pub fn run(target: &Target, options: Options) -> Result<()> {
    let ctx = Context::discover()?;
    for problem_id in ctx.repo.target_problems(target.problem_id, target.all)? {
        let client = ctx.client(problem_id)?;
        let dir = ctx.problem_dir(problem_id)?;
        push_one(&client, &dir, options)?;
    }
    Ok(())
}

fn push_one(client: &YukicoderClient, dir: &ProblemDir, options: Options) -> Result<()> {
    let problem_id = dir.problem_id();
    println!(
        "問題 {problem_id} を{}反映しています ({})",
        if options.dry_run { "(dry-run) " } else { "" },
        display_path(dir.root())
    );

    // ---- 問題設定と問題文 ----
    let settings = dir.read_settings()?;
    let judge_type = settings.judge_type;
    let statement = dir.read_statement()?;
    let request = ProblemEditRequest::new(settings, statement)?;
    if options.dry_run {
        println!(
            "  PUT /v1/problems/{problem_id}/edit\n{}",
            indent(&serde_json::to_string_pretty(&request)?, 4)
        );
    } else {
        let res: SaveResponse = client.save_problem_edit(problem_id, &request)?;
        report(&res, "  問題");
    }

    // ---- ジェネレータ ----
    if dir.has_generator() {
        let (config, source) = dir.read_generator()?;
        let request = GeneratorRequest {
            lang_id: config.lang_id,
            source,
            test_case_num: config.test_case_num,
            generate: options.generate.then_some(true),
            prefix: config.prefix,
        };
        if options.dry_run {
            println!(
                "  PUT /v1/problems/{problem_id}/generator (source {} バイト, {} ケース{})",
                request.source.len(),
                request.test_case_num,
                if options.generate {
                    ", 生成を起動"
                } else {
                    ""
                }
            );
        } else {
            let res = client.save_generator(problem_id, &request)?;
            report(&res, "  ジェネレータ");
        }
    }

    // ---- ジャッジコード (スペシャルジャッジ) ----
    if dir.has_judge_code() {
        push_judge_code(client, dir, judge_type, options)?;
    }

    // ---- 解説 ----
    // 外部URL一覧 (urlTable) は writer が操作するものではないので、同期の対象外。
    if dir.has_editorial() {
        let editorial = dir.read_editorial()?;
        let request = EditorialRequest::new(editorial)?;
        if options.dry_run {
            println!(
                "  PUT /v1/problems/{problem_id}/editorial ({} で {} バイト)",
                if request.markdown.is_some() {
                    "markdown"
                } else {
                    "html"
                },
                request.markdown.as_deref().unwrap_or(&request.html).len()
            );
        } else {
            let res = client.save_editorial(problem_id, &request)?;
            report(&res, "  解説");
        }
    }

    // ---- テストケース ----
    let mut testcases_changed = false;
    if options.testcases {
        // 名前の規則 (使える文字) はサーバに問い合わせる。
        let allowed_chars = client.testcase_name_rule()?;
        for which in [Which::In, Which::Out] {
            testcases_changed |= push_testcases(client, dir, which, &allowed_chars, options)?;
        }
    }

    // ---- validator ----
    // テストケースの後に処理する。テストケースを更新すると再検証が走るので、
    // ここでまとめて「送ったテストケースに対する結果」を 1 回の待ちで確認する。
    if dir.has_validator() {
        push_validator(client, dir, testcases_changed, options)?;
    }

    Ok(())
}

/// ジャッジコードを保存し、コンパイルの結果まで見届ける。
///
/// コンパイルが通らないジャッジコードを黙って残すと問題が壊れるので、CE なら
/// エラーにする。
fn push_judge_code(
    client: &YukicoderClient,
    dir: &ProblemDir,
    judge_type: i64,
    options: Options,
) -> Result<()> {
    let problem_id = dir.problem_id();
    let (config, source) = dir.read_judge_code()?;

    // ジャッジタイプが通常のままだと、保存はできても使われない。
    if judge_type == 0 {
        println!(
            "  ジャッジコード: 警告 problem.toml の judgeType が 0 (通常) です。\
             1 (スペシャル) などにしないとジャッジコードは使われません。"
        );
    }

    let request = JudgeCodeRequest {
        lang_id: config.lang_id,
        source,
    };
    if options.dry_run {
        println!(
            "  PUT /v1/problems/{problem_id}/code ({}, source {} バイト)",
            request.lang_id,
            request.source.len()
        );
        return Ok(());
    }

    // ソースが空だと削除になる。この場合コンパイルは走らず status も空のままなので、
    // 待つと必ず時間切れになる。
    let deleting = request.source.trim().is_empty();

    let res = client.save_judge_code(problem_id, &request)?;
    let message = res.message.trim();
    println!(
        "  ジャッジコード: {}",
        if message.is_empty() {
            if deleting {
                "削除しました"
            } else {
                "保存しました"
            }
        } else {
            message
        }
    );
    if deleting {
        return Ok(());
    }

    if !options.wait_compile {
        println!("  ジャッジコード: コンパイル結果は待ちません (--no-wait-compile)");
        return Ok(());
    }

    // 保存直後は WJ。判定中 (judging 分類) を抜けるまで待つ。
    let judging = judging_ids(&client.statuses()?);
    match wait_for_compile(client, problem_id, &judging)? {
        Some(code) if code.status == JUDGE_STATUS_OK => {
            println!("  ジャッジコード: コンパイル成功 ({})", code.status);
        }
        Some(code) => {
            let message = code.compile_message.trim().to_string();
            bail!(
                "ジャッジコードのコンパイルに失敗しました ({})。\
                 https://yukicoder.me/problems/{problem_id}/edit で内容を確認してください。{}",
                code.status,
                if message.is_empty() {
                    String::new()
                } else {
                    // 先頭 2000 バイトで切れることがある。
                    format!("\nコンパイルメッセージ (長いと途中で切れます):\n{message}")
                }
            )
        }
        None => println!(
            "  ジャッジコード: {COMPILE_TIMEOUT:?} 待ってもコンパイルが終わりませんでした。\
             `yuki-tool pull` で後から確認してください。"
        ),
    }
    Ok(())
}

/// validator を反映し、テストケースの検証結果まで見届ける。
///
/// テストケースの後に呼ぶ。テストケースを更新するとサーバが自動で再検証する
/// (API 経由は 10 秒のデバウンス付き) ので、「送ったテストケースに対する結果」
/// をここで 1 回の待ちにまとめて確認する。
///
/// サーバ負荷を抑えるため、ソースと言語に差分が無ければ PUT しない。PUT する
/// と検証がもう 1 回走るため、再検証と合わせて同じ検証が 2 回になってしまう。
fn push_validator(
    client: &YukicoderClient,
    dir: &ProblemDir,
    testcases_changed: bool,
    options: Options,
) -> Result<()> {
    let problem_id = dir.problem_id();
    let (config, source) = dir.read_validator()?;
    let remote = client.get_validator(problem_id)?;

    // ソースが空だと削除。両方空なら何もすることがない。
    let deleting = source.trim().is_empty();
    if deleting && remote.source.trim().is_empty() {
        println!("  validator: 未登録 (ローカルのソースも空)");
        return Ok(());
    }

    let unchanged = !deleting && remote.lang_id == config.lang_id && remote.source == source;

    if options.dry_run {
        if unchanged {
            println!("  validator: 差分なし (PUT しません)");
        } else {
            println!(
                "  PUT /v1/problems/{problem_id}/validator ({}, source {} バイト{})",
                config.lang_id,
                source.len(),
                if deleting { " → 削除" } else { "" }
            );
        }
        return Ok(());
    }

    // 非終端 (実行中・実行待ち) の判定は /v1/statuses の judging 分類を正と
    // する (語彙の列挙はクライアントに持たない)。
    let judging = judging_ids(&client.statuses()?);
    let judging = &judging;

    if unchanged {
        if !testcases_changed {
            if remote.is_up_to_date(judging) {
                if remote.status == JUDGE_STATUS_OK {
                    println!("  validator: 差分なし (検証状態: {})", remote.status);
                    return Ok(());
                }
                return fail_validation(problem_id, &remote);
            }
            println!("  validator: 差分はありませんが、検証が終わっていないので結果を待ちます");
        }
        // テストケースを変えた場合は、サーバの再検証 (デバウンス後に開始) を待つ。
    } else {
        let request = ValidatorRequest {
            lang_id: config.lang_id,
            source,
        };
        let res = client.save_validator(problem_id, &request)?;
        let message = res.message.trim();
        println!(
            "  validator: {}",
            if message.is_empty() {
                if deleting {
                    "削除しました"
                } else {
                    "保存しました"
                }
            } else {
                message
            }
        );
        // 削除では検証が走らず、DB の行ごと消えるので待たない。
        if deleting {
            return Ok(());
        }
    }

    if !options.wait_compile {
        println!("  validator: 検証結果は待ちません (--no-wait-compile)");
        return Ok(());
    }

    match wait_for_validation(client, problem_id, judging)? {
        Some(validator) if validator.status == JUDGE_STATUS_OK => {
            println!("  validator: 検証成功 ({})", validator.status);
            Ok(())
        }
        Some(validator) => fail_validation(problem_id, &validator),
        None => {
            println!(
                "  validator: {VALIDATION_TIMEOUT:?} 待っても検証が終わりませんでした。\
                 サーバの再起動などで再検証が走っていない可能性があります。\
                 時間をおいて `yuki-tool push` を実行し直してください。"
            );
            Ok(())
        }
    }
}

/// validator の検証失敗をエラーにする。壊れたテストケースや validator を
/// 黙って残さないため、push 自体を失敗させる。
fn fail_validation(
    problem_id: i64,
    validator: &crate::api::models::ValidatorContent,
) -> Result<()> {
    let status = validator.status.as_str();
    let what = match status {
        "WA" => "テストケースが validator を通りませんでした",
        "CE" => "validator のコンパイルに失敗しました",
        "RE" | "TLE" | "MLE" | "OLE" => "validator の実行に失敗しました",
        _ => "validator の検証に失敗しました",
    };
    bail!(
        "{what} ({status})。{}\n\
         https://yukicoder.me/problems/{problem_id}/validation で全体を確認できます。",
        validator.failure_details()
    )
}

/// 「今のテストケースに対する検証結果」が出るまで待つ。時間切れなら `None`。
///
/// 判定は [`crate::api::models::ValidatorContent::is_up_to_date`] (status が
/// judging でないこと)。テストケース更新と同時に `Pending` が立つことは
/// サーバ側が保証している。
fn wait_for_validation(
    client: &YukicoderClient,
    problem_id: i64,
    judging: &std::collections::HashSet<String>,
) -> Result<Option<crate::api::models::ValidatorContent>> {
    let deadline = Instant::now() + VALIDATION_TIMEOUT;
    loop {
        std::thread::sleep(VALIDATION_POLL_INTERVAL);
        let validator = client.get_validator(problem_id)?;
        if validator.is_up_to_date(judging) {
            return Ok(Some(validator));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
    }
}

/// コンパイル状態が判定中 (judging) を抜けるまで待つ。時間切れなら `None`。
fn wait_for_compile(
    client: &YukicoderClient,
    problem_id: i64,
    judging: &std::collections::HashSet<String>,
) -> Result<Option<crate::api::models::JudgeCodeContent>> {
    let deadline = Instant::now() + COMPILE_TIMEOUT;
    loop {
        std::thread::sleep(COMPILE_POLL_INTERVAL);
        let code = client.get_judge_code(problem_id)?;
        if !code.status.is_empty() && !judging.contains(code.status.as_str()) {
            return Ok(Some(code));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
    }
}

/// テストケースを反映する。サーバ側を実際に変更したら true を返す
/// (validator の再検証が走るかどうかの判断に使う)。
fn push_testcases(
    client: &YukicoderClient,
    dir: &ProblemDir,
    which: Which,
    allowed_chars: &str,
    options: Options,
) -> Result<bool> {
    let problem_id = dir.problem_id();
    let local = dir.read_testcases(which)?;
    for name in local.keys() {
        check_testcase_name(which, name, allowed_chars)?;
    }
    let details = client.list_testcases_detail(problem_id, which)?;
    let remote_names: Vec<&str> = details.iter().map(|d| d.name.as_str()).collect();

    // 一覧の sha256 は保存されているバイト列に対する値なので、ダウンロード
    // せずに比較できる。
    let mut changed_files: Vec<(String, Vec<u8>)> = Vec::new();
    for (name, content) in &local {
        let changed = details
            .iter()
            .find(|d| &d.name == name)
            .is_none_or(|d| d.sha256 != sha256_hex(content));
        if changed {
            changed_files.push((name.clone(), content.clone()));
        }
    }

    let uploaded = changed_files.len();
    for (name, _) in &changed_files {
        println!(
            "  テストケース {which}/{name}: アップロード{}",
            if options.dry_run {
                "予定"
            } else {
                "します"
            }
        );
    }
    if !options.dry_run {
        let mut uploaded_names: Vec<(String, Vec<u8>)> = Vec::new();
        for chunk in batches(changed_files) {
            let sent: Vec<String> = chunk.iter().map(|(name, _)| name.clone()).collect();
            uploaded_names.extend(chunk.iter().cloned());
            let res = client
                .upload_testcases(problem_id, which, chunk)
                .with_context(|| format!("テストケース {which} のアップロード"))?;
            if !res.warning.trim().is_empty() {
                println!("  テストケース {which}: 警告 {}", res.warning.trim());
            }
            // サーバはファイル名をサニタイズすることがある。名前が変わると
            // 次回も差分として残り続けるので、その場で知らせる。
            for (before, after) in sent.iter().zip(res.file_names.iter()) {
                if before != after {
                    println!(
                        "  テストケース {which}: {before} は {after} という名前で保存されました"
                    );
                }
            }
        }
        take_back_normalized(client, dir, which, &uploaded_names)?;
    }

    let mut pruned = 0usize;
    for name in remote_names {
        if local.contains_key(name) {
            continue;
        }
        if !options.prune {
            println!("  テストケース {which}/{name}: ローカルに無い (--prune で削除)");
            continue;
        }
        pruned += 1;
        if options.dry_run {
            println!("  テストケース {which}/{name}: 削除予定");
        } else {
            client
                .delete_testcase(problem_id, which, name)
                .with_context(|| format!("テストケース {which}/{name} の削除"))?;
            println!("  テストケース {which}/{name}: 削除しました");
        }
    }

    if uploaded == 0 && pruned == 0 {
        println!("  テストケース {which}: 差分なし ({} 件)", local.len());
    }
    Ok(!options.dry_run && (uploaded > 0 || pruned > 0))
}

/// yukicoder は保存時にテストケースを書き換える (改行コード、行末の半角
/// スペース、末尾の改行)。**その規則はクライアントに持たせず、保存された
/// 結果を取り込む。** 規則を写すと、サーバ側が変わったときに黙って食い違う。
///
/// 書き換えは保存と同期で行われるので、アップロード直後の一覧 (`?detail=1`)
/// に確定したハッシュが載る。送った内容とハッシュが違うファイルだけを取得
/// して書き戻す。
fn take_back_normalized(
    client: &YukicoderClient,
    dir: &ProblemDir,
    which: Which,
    uploaded: &[(String, Vec<u8>)],
) -> Result<()> {
    if uploaded.is_empty() {
        return Ok(());
    }
    let details = client.list_testcases_detail(dir.problem_id(), which)?;
    let adjusted = uploaded.iter().filter(|(name, sent)| {
        details
            .iter()
            .find(|d| &d.name == name)
            .is_none_or(|d| d.sha256 != sha256_hex(sent))
    });
    for (name, sent) in adjusted {
        let stored = client
            .get_testcase(dir.problem_id(), which, name)
            .with_context(|| format!("テストケース {which}/{name} の取得"))?;
        if &stored == sent {
            continue;
        }
        dir.write_testcase(which, name, &stored)?;
        println!(
            "  テストケース {which}/{name}: yukicoder 側で内容が調整されたので \
             {} を更新しました",
            display_path(dir.testcase_path(which, name))
        );
    }
    Ok(())
}

/// validator の検証を待つ時間。
///
/// テストケース更新の再検証は 10 秒のデバウンスの後に始まり、そこから全テスト
/// ケースを実行するので、コンパイルより長めに取る。
const VALIDATION_TIMEOUT: Duration = Duration::from_secs(600);
const VALIDATION_POLL_INTERVAL: Duration = Duration::from_secs(5);

/// アップロードは HTTP ヘッダ込みで 30MiB までなので、余裕を見て分割する。
/// ジャッジコードのコンパイルを待つ時間。
const COMPILE_TIMEOUT: Duration = Duration::from_secs(180);
const COMPILE_POLL_INTERVAL: Duration = Duration::from_secs(3);

const UPLOAD_CHUNK_BYTES: usize = 20 * 1024 * 1024;
/// 1 リクエストあたりのファイル数の上限。
const UPLOAD_CHUNK_FILES: usize = 100;

fn batches(files: Vec<(String, Vec<u8>)>) -> Vec<Vec<(String, Vec<u8>)>> {
    let mut batches = Vec::new();
    let mut current: Vec<(String, Vec<u8>)> = Vec::new();
    let mut size = 0usize;
    for (name, content) in files {
        let entry = content.len() + name.len() + 256; // multipart のヘッダ分
        if !current.is_empty()
            && (size + entry > UPLOAD_CHUNK_BYTES || current.len() >= UPLOAD_CHUNK_FILES)
        {
            batches.push(std::mem::take(&mut current));
            size = 0;
        }
        size += entry;
        current.push((name, content));
    }
    if !current.is_empty() {
        batches.push(current);
    }
    batches
}

fn report(res: &SaveResponse, what: &str) {
    if res.message.trim().is_empty() {
        println!("{what}: 保存しました");
    } else {
        println!("{what}: {}", res.message.trim());
    }
}

fn indent(text: &str, spaces: usize) -> String {
    let pad = " ".repeat(spaces);
    text.lines()
        .map(|line| format!("{pad}{line}"))
        .collect::<Vec<_>>()
        .join("\n")
}
