//! `yuki testcases` — yukicoder 側のテストケース一覧を表示する。

use anyhow::Result;

use crate::api::models::Which;
use crate::commands::Context;
use crate::Target;

pub fn list(target: &Target, which: Option<Which>) -> Result<()> {
    let ctx = Context::discover()?;
    let kinds: Vec<Which> = match which {
        Some(w) => vec![w],
        None => vec![Which::In, Which::Out],
    };
    for problem_id in ctx.repo.target_problems(target.problem_id, target.all)? {
        let client = ctx.client(problem_id)?;
        println!("--- 問題 {problem_id} ---");
        for w in &kinds {
            let names = client.list_testcases(problem_id, *w)?;
            println!("{w}: {} 件", names.len());
            for name in names {
                println!("  {name}");
            }
        }
    }
    Ok(())
}
