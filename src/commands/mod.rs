pub mod diff;
pub mod init;
pub mod languages;
pub mod new;
pub mod pull;
pub mod push;
pub mod submit;
pub mod testcases;

use anyhow::Result;

use crate::api::YukicoderClient;
use crate::config::{self, Repo};
use crate::local::ProblemDir;

/// リポジトリを見つけて、問題ごとのクライアントを作れるようにする。
pub struct Context {
    pub repo: Repo,
}

impl Context {
    pub fn discover() -> Result<Self> {
        Ok(Self {
            repo: Repo::discover()?,
        })
    }

    /// 問題ごとにトークンを解決してクライアントを作る。
    ///
    /// 編集トークンは 1 つの問題にしか使えないので、問題ごとに作り直す。
    pub fn client(&self, problem_id: i64) -> Result<YukicoderClient> {
        let token = config::resolve_token(&self.repo.root, problem_id)?;
        YukicoderClient::new(token, self.repo.config.base_url.clone())
    }

    pub fn problem_dir(&self, problem_id: i64) -> Result<ProblemDir> {
        Ok(ProblemDir::new(
            self.repo.problem_dir(problem_id)?,
            problem_id,
        ))
    }
}
