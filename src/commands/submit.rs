//! `yuki submit` / `yuki solution` — 提出と、想定解の登録。
//!
//! 編集トークンで提出すると、トークンの発行者としての提出になる。

use std::path::Path;

use anyhow::{Context as _, Result};

use crate::api::models::SolutionRequest;
use crate::commands::Context;
use crate::local::{display_path, read_text};

pub fn run(problem_id: Option<i64>, file: &Path, lang: &str) -> Result<()> {
    let ctx = Context::discover()?;
    let problem_id = single(&ctx, problem_id)?;
    let source = read_text(file)?;
    if source.trim().is_empty() {
        anyhow::bail!("{} が空です", display_path(file));
    }

    let client = ctx.client(problem_id)?;
    println!(
        "問題 {problem_id} に {} を提出しています ({lang}, {} バイト)",
        display_path(file),
        source.len()
    );
    let response = client.submit(problem_id, lang, source)?;
    let response = response.trim();

    match submission_id(response) {
        Some(id) => println!(
            "提出しました: 提出 ID {id}\n結果は https://yukicoder.me/submissions/{id} で確認できます。\n\
             AC を確認したら `yuki solution {id} --summary \"...\"` で想定解に登録できます。"
        ),
        None => println!("提出しました。レスポンス: {response}"),
    }
    Ok(())
}

pub fn set_solution(
    submission_id: i64,
    summary: Option<String>,
    delete: bool,
    problem_id: Option<i64>,
) -> Result<()> {
    let ctx = Context::discover()?;
    let problem_id = single(&ctx, problem_id)?;
    let client = ctx.client(problem_id)?;
    let request = SolutionRequest {
        summary: if delete { None } else { summary },
        delete: delete.then_some(true),
    };
    let res = client.set_solution(submission_id, &request)?;
    let message = res.message.trim();
    println!(
        "提出 {submission_id}: {}",
        if message.is_empty() {
            if delete {
                "想定解の登録を消しました"
            } else {
                "想定解に登録しました"
            }
        } else {
            message
        }
    );
    Ok(())
}

/// 提出・想定解の登録は 1 問だけを相手にする。
fn single(ctx: &Context, problem_id: Option<i64>) -> Result<i64> {
    let ids = ctx.repo.target_problems(problem_id, false)?;
    ids.first()
        .copied()
        .context("対象の問題 ID を決められませんでした")
}

/// 提出 API のレスポンスから提出 ID を拾う。
///
/// swagger 上は `application/octet-stream` で形式が決まっていないため、
/// JSON でも数値だけでも受け取れるようにしておく。
fn submission_id(response: &str) -> Option<i64> {
    if let Ok(id) = response.parse::<i64>() {
        return Some(id);
    }
    let value: serde_json::Value = serde_json::from_str(response).ok()?;
    for key in ["SubmissionId", "submissionId", "Id", "id"] {
        match value.get(key) {
            Some(serde_json::Value::Number(n)) => return n.as_i64(),
            Some(serde_json::Value::String(s)) => {
                if let Ok(id) = s.parse::<i64>() {
                    return Some(id);
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::submission_id;

    #[test]
    fn parses_plain_number() {
        assert_eq!(submission_id("1234567"), Some(1234567));
    }

    #[test]
    fn parses_json_shapes() {
        assert_eq!(submission_id(r#"{"SubmissionId":42}"#), Some(42));
        assert_eq!(submission_id(r#"{"submissionId":"42"}"#), Some(42));
        assert_eq!(submission_id(r#"{"Id":7,"Message":"ok"}"#), Some(7));
    }

    #[test]
    fn returns_none_for_unknown_shape() {
        assert_eq!(submission_id(r#"{"Message":"受け付けました"}"#), None);
        assert_eq!(submission_id("ok"), None);
    }
}
