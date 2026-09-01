//! `yuki-tool` — Git リポジトリから yukicoder の問題を管理する CLI。
//!
//! トークンは環境変数か `.env` からだけ読む (コマンドライン引数では渡さない)。

mod api;
mod commands;
mod config;
mod local;

use anyhow::Result;
use clap::{Args, Parser, Subcommand};

use api::models::Which;

/// 差分があったときの終了コード (`diff --exit-code`)。
pub const EXIT_DIFF: i32 = 2;

#[derive(Debug, Parser)]
#[command(
    name = "yuki-tool",
    version,
    about = "yukicoder の問題を Git リポジトリから管理する",
    long_about = "yukicoder の公開 API を使って、問題設定・問題文・テストケース・\
ジェネレータ・解説をローカルのファイルと同期する。\n\
トークンは環境変数 (YUKICODER_TOKEN_<問題ID> / YUKICODER_TOKEN / YUKICODER_API_KEY) \
か、リポジトリ直下の .env から読む。"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// yukicoder.toml を作り、問題を取得する
    Init {
        /// 問題 ID
        problem_id: i64,
        /// テストケースは取得しない
        #[arg(long)]
        no_testcases: bool,
    },
    /// 問題のディレクトリ一式を作る (yukicoder で問題を作った直後に使う)
    New {
        /// 問題 ID
        problem_id: i64,
        /// 置き場所 (problems ディレクトリからの相対。省略時は問題 ID)
        #[arg(long)]
        dir: Option<String>,
    },
    /// yukicoder の内容をローカルに取得する
    Pull {
        #[command(flatten)]
        target: Target,
        /// テストケースは取得しない
        #[arg(long)]
        no_testcases: bool,
    },
    /// ローカルの内容を yukicoder に反映する
    Push {
        #[command(flatten)]
        target: Target,
        /// 送信内容を表示するだけで、実際には送らない
        #[arg(long)]
        dry_run: bool,
        /// テストケースは反映しない
        #[arg(long)]
        no_testcases: bool,
        /// ローカルに無いテストケースをサーバから削除する
        #[arg(long)]
        prune: bool,
        /// ジェネレータの保存後にケース生成を起動する
        #[arg(long)]
        generate: bool,
        /// ジャッジコードのコンパイル結果を待たない
        #[arg(long)]
        no_wait_compile: bool,
    },
    /// ローカルと yukicoder の差分を表示する
    Diff {
        #[command(flatten)]
        target: Target,
        /// テストケースは比較しない
        #[arg(long)]
        no_testcases: bool,
        /// 差分があれば終了コード 2 で終わる (CI 用)
        #[arg(long)]
        exit_code: bool,
    },
    /// ソースを提出する
    Submit {
        /// 問題 ID
        problem_id: Option<i64>,
        /// 提出するファイル
        #[arg(short, long)]
        file: std::path::PathBuf,
        /// 言語 ID (`yuki-tool languages` で確認)
        #[arg(short, long)]
        lang: String,
    },
    /// 提出を解説ページの「想定解」として登録する / 登録を消す
    Solution {
        /// 提出 ID
        submission_id: i64,
        /// 想定解の説明
        #[arg(long)]
        summary: Option<String>,
        /// 登録を消す
        #[arg(long)]
        delete: bool,
        /// 認証に使う問題 ID (編集トークンを問題ごとに分けている場合)
        #[arg(long)]
        problem_id: Option<i64>,
    },
    /// テストケースの一覧を表示する
    Testcases {
        #[command(flatten)]
        target: Target,
        /// in / out のどちらか (省略すると両方)
        #[arg(long, value_enum)]
        which: Option<Which>,
    },
    /// 使える言語の一覧を表示する
    Languages {
        /// 現在使えない言語も表示する
        #[arg(long)]
        include_disabled: bool,
    },
}

/// 対象の問題を指定する共通の引数。
#[derive(Debug, Args)]
pub struct Target {
    /// 問題 ID (省略時は、リポジトリに問題が 1 つならそれ)
    pub problem_id: Option<i64>,
    /// リポジトリにあるすべての問題を対象にする
    #[arg(long)]
    pub all: bool,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("エラー: {err:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Init {
            problem_id,
            no_testcases,
        } => commands::init::run(problem_id, !no_testcases),
        Command::New { problem_id, dir } => commands::new::run(problem_id, dir),
        Command::Pull {
            target,
            no_testcases,
        } => commands::pull::run(&target, !no_testcases),
        Command::Push {
            target,
            dry_run,
            no_testcases,
            prune,
            generate,
            no_wait_compile,
        } => commands::push::run(
            &target,
            commands::push::Options {
                dry_run,
                testcases: !no_testcases,
                prune,
                generate,
                wait_compile: !no_wait_compile,
            },
        ),
        Command::Diff {
            target,
            no_testcases,
            exit_code,
        } => commands::diff::run(&target, !no_testcases, exit_code),
        Command::Submit {
            problem_id,
            file,
            lang,
        } => commands::submit::run(problem_id, &file, &lang),
        Command::Solution {
            submission_id,
            summary,
            delete,
            problem_id,
        } => commands::submit::set_solution(submission_id, summary, delete, problem_id),
        Command::Testcases { target, which } => commands::testcases::list(&target, which),
        Command::Languages { include_disabled } => commands::languages::run(include_disabled),
    }
}
