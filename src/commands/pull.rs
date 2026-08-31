//! `yuki pull` — yukicoder の内容をローカルのファイルに書き出す。

use anyhow::{Context as _, Result};

use crate::api::models::{Statement, Which};
use crate::api::YukicoderClient;
use crate::commands::Context;
use crate::local::{display_path, GeneratorConfig, JudgeConfig, ProblemDir};
use crate::Target;

pub fn run(target: &Target, testcases: bool) -> Result<()> {
    let ctx = Context::discover()?;
    for problem_id in ctx.repo.target_problems(target.problem_id, target.all)? {
        let client = ctx.client(problem_id)?;
        let dir = ctx.problem_dir(problem_id);
        pull_one(&client, &dir, testcases)?;
    }
    Ok(())
}

pub fn pull_one(client: &YukicoderClient, dir: &ProblemDir, testcases: bool) -> Result<()> {
    let problem_id = dir.problem_id();
    println!(
        "問題 {problem_id} を取得しています ({})",
        display_path(dir.root())
    );

    let remote = client.get_problem_edit(problem_id)?;
    if remote.problem_id != problem_id {
        anyhow::bail!(
            "問題 {problem_id} を要求したのに、問題 {} が返りました",
            remote.problem_id
        );
    }
    dir.write_settings(&remote.settings)?;
    let statement = Statement::new(remote.content, remote.is_markdown);
    dir.write_statement(&statement)?;
    println!(
        "  問題設定 -> {}\n  問題文   -> {}{}",
        display_path(dir.settings_path()),
        display_path(dir.statement_path(statement.is_markdown())),
        if remote.showable {
            ""
        } else {
            " (この問題は未公開です)"
        }
    );

    // ジェネレータは未登録なら langId も source も空で返る。空のときは
    // ローカルにファイルを作らない。
    let generator = client.get_generator(problem_id)?;
    if generator.source.trim().is_empty() {
        println!("  ジェネレータ: 未登録");
    } else {
        let source_file = existing_generator_source_file(dir)
            .unwrap_or_else(|| source_file_name("generator", &generator.lang_id));
        let config = GeneratorConfig {
            lang_id: generator.lang_id.clone(),
            source_file: source_file.clone(),
            test_case_num: generator.test_case_num,
            prefix: None,
        };
        dir.write_generator(&config, &generator.source)?;
        println!(
            "  ジェネレータ -> {} (生成有効: {})",
            display_path(dir.generator_dir().join(&source_file)),
            if generator.enable {
                "はい"
            } else {
                "いいえ"
            }
        );
    }

    // ジャッジコード (スペシャルジャッジ)。API がまだ生えていないサーバでは
    // None が返るので、その場合はほかの同期を止めずに知らせるだけにする。
    match client.get_judge_code(problem_id)? {
        None => println!("  ジャッジコード: この API はまだ使えません (サーバ側が未対応)"),
        Some(code) if code.source.trim().is_empty() => println!("  ジャッジコード: 未登録"),
        Some(code) => {
            let source_file = existing_judge_source_file(dir)
                .unwrap_or_else(|| source_file_name("judge", &code.lang_id));
            let config = JudgeConfig {
                lang_id: code.lang_id.clone(),
                source_file: source_file.clone(),
            };
            dir.write_judge_code(&config, &code.source)?;
            println!(
                "  ジャッジコード -> {} (コンパイル状態: {})",
                display_path(dir.judge_dir().join(&source_file)),
                if code.status.is_empty() {
                    "不明"
                } else {
                    &code.status
                }
            );
        }
    }

    // 解説は未作成でもテンプレートが返る。すでにローカルにある場合だけ更新し、
    // 無い場合は作らない (テンプレートを毎回コミットしないため)。
    let editorial = client.get_editorial(problem_id)?;
    if dir.has_editorial() {
        let statement = Statement::new(editorial.content, editorial.is_markdown);
        dir.write_editorial(&statement)?;
        println!(
            "  解説 -> {}",
            display_path(dir.editorial_path(statement.is_markdown()))
        );
    } else {
        println!(
            "  解説: ローカルに {} が無いので取得しません",
            display_path(dir.editorial_path(true))
        );
    }

    if testcases {
        for which in [Which::In, Which::Out] {
            let names = client.list_testcases(problem_id, which)?;
            for name in &names {
                let content = client
                    .get_testcase(problem_id, which, name)
                    .with_context(|| format!("テストケース {which}/{name} の取得"))?;
                dir.write_testcase(which, name, &content)?;
            }
            println!("  テストケース {which}: {} 件", names.len());
        }
    }

    Ok(())
}

/// すでにローカルにあるジェネレータのソースファイル名を使い回す。
fn existing_generator_source_file(dir: &ProblemDir) -> Option<String> {
    dir.read_generator()
        .ok()
        .map(|(config, _)| config.source_file)
}

/// すでにローカルにあるジャッジコードのソースファイル名を使い回す。
fn existing_judge_source_file(dir: &ProblemDir) -> Option<String> {
    dir.read_judge_code()
        .ok()
        .map(|(config, _)| config.source_file)
}

/// 初めて取得したときのソースファイル名。言語 ID から拡張子を決める。
fn source_file_name(stem: &str, lang_id: &str) -> String {
    let ext = match lang_id {
        id if id.starts_with("cpp") => "cpp",
        id if id.starts_with("python") || id.starts_with("pypy") => "py",
        id if id.starts_with("rust") => "rs",
        id if id.starts_with("go") => "go",
        id if id.starts_with("java") => "java",
        _ => "txt",
    };
    format!("{stem}.{ext}")
}
