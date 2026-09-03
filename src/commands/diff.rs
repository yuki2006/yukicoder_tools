//! `yuki-tool diff` — ローカルと yukicoder の差分を表示する。
//!
//! CI の PR チェック用に `--exit-code` を用意している。差分があれば終了コード 2。

use anyhow::Result;
use similar::TextDiff;

use crate::api::models::{ProblemSettings, Statement, Which};
use crate::api::YukicoderClient;
use crate::commands::Context;
use crate::local::ProblemDir;
use crate::{Target, EXIT_DIFF};

pub fn run(target: &Target, testcases: bool, exit_code: bool) -> Result<()> {
    let ctx = Context::discover()?;
    let mut differs = false;
    for problem_id in ctx.repo.target_problems(target.problem_id, target.all)? {
        let client = ctx.client(problem_id)?;
        let dir = ctx.problem_dir(problem_id)?;
        differs |= diff_one(&client, &dir, testcases)?;
    }
    if differs && exit_code {
        std::process::exit(EXIT_DIFF);
    }
    Ok(())
}

/// 差分があれば true。
fn diff_one(client: &YukicoderClient, dir: &ProblemDir, testcases: bool) -> Result<bool> {
    let problem_id = dir.problem_id();
    println!("--- 問題 {problem_id} ---");
    let mut differs = false;

    let remote = client.get_problem_edit(problem_id)?;

    let local_settings = dir.read_settings()?;
    let changes = settings_changes(&remote.settings, &local_settings)?;
    if changes.is_empty() {
        println!("問題設定: 差分なし");
    } else {
        differs = true;
        println!("問題設定:");
        for (key, remote_value, local_value) in changes {
            println!("  {key}: {remote_value} -> {local_value}");
        }
    }

    let local_statement = dir.read_statement()?;
    let remote_statement = Statement::new(remote.content, remote.is_markdown);
    if remote_statement.is_markdown() != local_statement.is_markdown() {
        differs = true;
        println!(
            "問題文: 形式が違います (yukicoder: {}, ローカル: {})",
            format_kind(remote_statement.is_markdown()),
            format_kind(local_statement.is_markdown())
        );
    }
    differs |= print_text_diff("問題文", remote_statement.text(), local_statement.text());

    if dir.has_editorial() {
        let editorial = client.get_editorial(problem_id)?;
        let local_editorial = dir.read_editorial()?;
        // push はローカルの拡張子で保存形式を送るので、形式の違いも差分。
        if editorial.is_markdown != local_editorial.is_markdown() {
            differs = true;
            println!(
                "解説: 形式が違います (yukicoder: {}, ローカル: {})",
                format_kind(editorial.is_markdown),
                format_kind(local_editorial.is_markdown())
            );
        }
        differs |= print_text_diff("解説", &editorial.content, local_editorial.text());
    }

    if dir.has_generator() {
        let remote_generator = client.get_generator(problem_id)?;
        let (config, source) = dir.read_generator()?;
        if remote_generator.lang_id != config.lang_id {
            differs = true;
            println!(
                "ジェネレータ langId: {} -> {}",
                remote_generator.lang_id, config.lang_id
            );
        }
        if remote_generator.test_case_num != config.test_case_num {
            differs = true;
            println!(
                "ジェネレータ testCaseNum: {} -> {}",
                remote_generator.test_case_num, config.test_case_num
            );
        }
        differs |= print_text_diff("ジェネレータ", &remote_generator.source, &source);
    }

    if dir.has_judge_code() {
        let (config, source) = dir.read_judge_code()?;
        match client.get_judge_code(problem_id)? {
            None => println!("ジャッジコード: このサーバはジャッジコードの API に対応していません"),
            Some(remote) => {
                if remote.lang_id != config.lang_id {
                    differs = true;
                    println!(
                        "ジャッジコード langId: {} -> {}",
                        remote.lang_id, config.lang_id
                    );
                }
                differs |= print_text_diff("ジャッジコード", &remote.source, &source);
            }
        }
    }

    if testcases {
        for which in [Which::In, Which::Out] {
            differs |= diff_testcases(client, dir, which)?;
        }
    }

    Ok(differs)
}

fn diff_testcases(client: &YukicoderClient, dir: &ProblemDir, which: Which) -> Result<bool> {
    let problem_id = dir.problem_id();
    let local = dir.read_testcases(which)?;
    let remote_names = client.list_testcases(problem_id, which)?;
    let mut differs = false;

    for name in &remote_names {
        if !local.contains_key(name) {
            differs = true;
            println!("テストケース {which}/{name}: yukicoder にのみ存在");
        }
    }
    for (name, content) in &local {
        if !remote_names.iter().any(|n| n == name) {
            differs = true;
            println!("テストケース {which}/{name}: ローカルにのみ存在");
            continue;
        }
        let remote = client.get_testcase(problem_id, which, name)?;
        if &remote != content {
            differs = true;
            println!("テストケース {which}/{name}: 内容が違います");
        }
    }
    if !differs {
        println!("テストケース {which}: 差分なし ({} 件)", local.len());
    }
    Ok(differs)
}

/// 設定を JSON に落として、キーごとに比べる。
///
/// フィールドを増やしたときにここを直し忘れないよう、構造体の定義から導く。
fn settings_changes(
    remote: &ProblemSettings,
    local: &ProblemSettings,
) -> Result<Vec<(String, String, String)>> {
    let remote = serde_json::to_value(remote)?;
    let local = serde_json::to_value(local)?;
    let (Some(remote), Some(local)) = (remote.as_object(), local.as_object()) else {
        return Ok(Vec::new());
    };
    let mut changes = Vec::new();
    for (key, remote_value) in remote {
        let local_value = local.get(key);
        if local_value != Some(remote_value) {
            changes.push((
                key.clone(),
                display_value(key, remote_value),
                local_value
                    .map(|v| display_value(key, v))
                    .unwrap_or_default(),
            ));
        }
    }
    Ok(changes)
}

/// 数値コードには名前を添える (例: `1 (スペシャル)`)。
fn display_value(key: &str, value: &serde_json::Value) -> String {
    use crate::api::models::{eps_mode_label, judge_type_label, problem_type_label};
    let label = match key {
        "judgeType" => value.as_i64().and_then(judge_type_label),
        "problemType" => value.as_i64().and_then(problem_type_label),
        "epsMode" => value.as_str().and_then(eps_mode_label),
        _ => None,
    };
    match label {
        Some(label) => format!("{value} ({label})"),
        None => value.to_string(),
    }
}

/// 差分があれば表示して true を返す。
fn print_text_diff(what: &str, remote: &str, local: &str) -> bool {
    if remote == local {
        println!("{what}: 差分なし");
        return false;
    }
    println!("{what}:");
    let diff = TextDiff::from_lines(remote, local);
    print!(
        "{}",
        diff.unified_diff()
            .context_radius(3)
            .header("yukicoder", "ローカル")
    );
    true
}

fn format_kind(is_markdown: bool) -> &'static str {
    if is_markdown {
        "Markdown"
    } else {
        "HTML"
    }
}
