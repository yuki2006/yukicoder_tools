//! `yuki push` — ローカルのファイルを yukicoder に反映する。
//!
//! 書き込みは PUT (全置換)。部分更新ではないので問題設定は毎回すべて送る。問題文が空のまま
//! 送ると保存済みの問題文が消えるため、`ProblemEditRequest::new` で止める。

use std::time::{Duration, Instant};

use anyhow::{bail, Context as _, Result};

use crate::api::models::{
    judge_status_is_final, EditorialRequest, GeneratorRequest, JudgeCodeRequest,
    ProblemEditRequest, SaveResponse, Which, JUDGE_STATUS_OK,
};
use crate::api::YukicoderClient;
use crate::commands::Context;
use crate::local::{display_path, ProblemDir};
use crate::Target;

#[derive(Debug, Clone, Copy)]
pub struct Options {
    pub dry_run: bool,
    pub testcases: bool,
    /// ローカルに無いテストケースをサーバから消す。
    pub prune: bool,
    /// ジェネレータ保存後にケース生成を起動する。
    pub generate: bool,
    /// ジャッジコードのコンパイル結果を待つ。
    pub wait_compile: bool,
}

pub fn run(target: &Target, options: Options) -> Result<()> {
    let ctx = Context::discover()?;
    for problem_id in ctx.repo.target_problems(target.problem_id, target.all)? {
        let client = ctx.client(problem_id)?;
        let dir = ctx.problem_dir(problem_id);
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
    // 外部URL一覧 (urlTable) は送らない。PUT では既存分に追記されるだけで置換
    // できず、毎回送ると同じ行が増えるため。登録は WebUI で行う。
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
    if options.testcases {
        for which in [Which::In, Which::Out] {
            push_testcases(client, dir, which, options)?;
        }
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

    // ジャッジタイプが標準のままだと、保存はできても使われない。
    if judge_type == 0 {
        println!(
            "  ジャッジコード: 警告 problem.toml の judgeType が 0 (標準) です。\
             1 (スペシャル) 以上にしないとジャッジコードは使われません。"
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

    // 保存直後は WJ。コンパイルが終わると AC か CE になる。
    let status = if judge_status_is_final(&res.status) {
        Some(res.status.clone())
    } else if options.wait_compile {
        wait_for_compile(client, problem_id)?
    } else {
        None
    };

    match status {
        Some(status) if status == JUDGE_STATUS_OK => {
            println!("  ジャッジコード: コンパイル成功 ({status})");
        }
        Some(status) => bail!(
            "ジャッジコードのコンパイルに失敗しました ({status})。\
             https://yukicoder.me/problems/{problem_id}/edit で内容を確認してください。"
        ),
        None if options.wait_compile => println!(
            "  ジャッジコード: {COMPILE_TIMEOUT:?} 待ってもコンパイルが終わりませんでした。\
             `yuki pull` で後から確認してください。"
        ),
        None => println!("  ジャッジコード: コンパイル結果は待ちません (--no-wait-compile)"),
    }
    Ok(())
}

/// コンパイル状態が確定するまで待つ。時間切れなら `None`。
///
/// `WJ` と `Judge` は途中の状態なので、確定するまで待ち続ける。
fn wait_for_compile(client: &YukicoderClient, problem_id: i64) -> Result<Option<String>> {
    let deadline = Instant::now() + COMPILE_TIMEOUT;
    loop {
        std::thread::sleep(COMPILE_POLL_INTERVAL);
        if let Some(code) = client.get_judge_code(problem_id)? {
            if judge_status_is_final(&code.status) {
                return Ok(Some(code.status));
            }
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
    }
}

fn push_testcases(
    client: &YukicoderClient,
    dir: &ProblemDir,
    which: Which,
    options: Options,
) -> Result<()> {
    let problem_id = dir.problem_id();
    let local = dir.read_testcases(which)?;
    let remote_names = client.list_testcases(problem_id, which)?;

    let mut changed_files: Vec<(String, Vec<u8>)> = Vec::new();
    for (name, content) in &local {
        let changed = if remote_names.iter().any(|n| n == name) {
            let remote = client
                .get_testcase(problem_id, which, name)
                .with_context(|| format!("テストケース {which}/{name} の取得"))?;
            &remote != content
        } else {
            true
        };
        if changed {
            changed_files.push((name.clone(), content.clone().into_bytes()));
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
        for chunk in batches(changed_files) {
            let sent: Vec<String> = chunk.iter().map(|(name, _)| name.clone()).collect();
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
    }

    let mut pruned = 0usize;
    for name in &remote_names {
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
    Ok(())
}

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
