# yukicoder-tools

yukicoder の問題を Git リポジトリで管理し、CI/CD から反映するための CLI (`yuki`) です。

問題設定・問題文・テストケース・ジェネレータ・解説をローカルのファイルとして扱い、
yukicoder の公開 API 経由で双方向に同期します。リポジトリを正として、
main にマージされた内容を GitHub Actions が yukicoder へ反映する、という運用を想定しています。

## できること

| コマンド | 内容 |
| --- | --- |
| `yuki init <問題ID>` | `yukicoder.toml` を作り、問題を取得する |
| `yuki pull [問題ID]` | yukicoder の内容をローカルに書き出す |
| `yuki diff [問題ID]` | ローカルと yukicoder の差分を表示する (`--exit-code` で差分時に終了コード 2) |
| `yuki push [問題ID]` | ローカルの内容を yukicoder に反映する (`--dry-run` / `--prune` / `--generate`) |
| `yuki submit --file <ファイル> --lang <言語ID>` | ソースを提出する |
| `yuki solution <提出ID> --summary "..."` | AC した提出を解説ページの「想定解」に登録する (`--delete` で解除) |
| `yuki testcases` | yukicoder 側のテストケース一覧を表示する |
| `yuki languages` | 提出・ジェネレータで使う言語 ID の一覧 |

`pull` / `diff` / `push` は `--all` で `yukicoder.toml` に書いた問題すべてを対象にできます。

## インストール

```sh
cargo build --release
# target/release/yuki
```

## トークン

問題の管理画面で発行する編集トークン (`ypt_...`) か、アカウントの API キーを使います。
**コマンドライン引数では渡しません。** CI のログやプロセス一覧に残さないためです。
次の順に探し、最初に見つかったものを使います。

1. `YUKICODER_TOKEN_<問題ID>` — 問題ごとの編集トークン。複数の問題を扱うときはこの形
2. `YUKICODER_TOKEN`
3. `YUKICODER_API_KEY` — アカウントの API キー。作者・テスターならどの問題にも使える

それぞれ、環境変数を先に見て、無ければリポジトリ直下の `.env` を見ます
(GitHub Actions の Secrets が `.env` より優先されます)。どれも無ければエラーで止まります。

ローカルでは `.env.example` を `.env` にコピーして使ってください。`.env` は `.gitignore` 済みです。
GitHub Actions では Secrets に `YUKICODER_TOKEN` を登録し、ジョブの `env` に渡します。

## ディレクトリ構成

```text
yukicoder.toml              管理対象の問題 ID / API のベース URL
problems/<問題ID>/
  problem.toml              問題設定 (キー名は API と同じ camelCase)
  statement.md              問題文。HTML で管理する問題は statement.html
  editorial.md              解説 (任意)。HTML なら editorial.html
  editorial_urls.txt        解説の外部URL一覧 (参照用。push では送らない)
  judge/
    judge.toml              スペシャルジャッジのジャッジコードの設定 (langId / sourceFile)
    <sourceFile>            ジャッジコードのソース
  generator/
    generator.toml          langId / sourceFile / testCaseNum / prefix
    <sourceFile>            ジェネレータのソース
  testcases/
    in/*.txt
    out/*.txt
  solutions/                提出用のソース (同期対象ではない)
```

- 問題文の形式は拡張子で決まります。`statement.md` があれば Markdown、`statement.html` があれば HTML。
  両方あるとどちらを送るか決められないのでエラーにします。
- 解説はローカルに `editorial.md` / `editorial.html` があるときだけ同期します。
  未作成の解説は API がテンプレートを返すので、それを毎回コミットしないためです。
- 改行は読み書きとも LF に正規化します。`.gitattributes` で `eol=lf` を指定しています。

### スペシャルジャッジ (ジャッジコード)

`judge/judge.toml` があるときだけ同期します。`push` は保存後にコンパイル結果を待ち、
`CE` なら終了コード 1 で失敗します (コンパイルの通らないジャッジコードを黙って残さないため)。
待たずに進めるときは `--no-wait-compile` を付けます。

コンパイル状態は `WJ` (待機) → `Judge` (コンパイル中) → `AC` / `CE` と遷移します。
**確定するのは `AC` と `CE` だけ**で、途中の状態を失敗として扱わないようにしています。
ソースを空にすると削除になり、この場合はコンパイルが走らないので待ちません。

`problem.toml` の `judgeType` が 0 (標準) のままだと、ジャッジコードは保存できても使われません。
`judgeType` を 1 (スペシャル) 以上にしてください。`push` は問題設定を先に送るので、
同じコミットで両方を変えれば 1 回で反映されます。

## CI/CD

手順と実際の出力は [docs/github-actions.md](docs/github-actions.md) にまとめています。
`.github/workflows/` に 4 つ用意しています。

- `ci.yml` — `cargo fmt --check` / `clippy -D warnings` / `test`
- `problem-diff.yml` — PR で `problems/**` を触ったとき、yukicoder との差分をジョブサマリに出す。
  差分があること自体は失敗にしない (PR は「これから反映する変更」なので)。
  フォークからの PR には Secrets が渡らないので実行しない
- `problem-push.yml` — main に入ったら `yuki push --all` で反映する。
  `workflow_dispatch` から `--prune` / `--dry-run` も選べる
- `problem-pull.yml` — 毎日 (と手動で) `yuki pull --all` し、yukicoder 側に
  リポジトリと違う内容があれば取り込む PR を作る

反映は last-write-wins です。WebUI 側で直接編集した内容は push で上書きされます。
WebUI で編集したときは、`problem-pull.yml` が作る PR を先にマージしてください。

> 未公開 (WIP) の問題を扱う場合、`problems/` を公開リポジトリに置くと問題文が公開されます。
> 問題を管理するリポジトリは private にするか、ツール本体と問題データでリポジトリを分けてください。

## API の扱いで注意している点

実際の API の挙動に合わせています (2026-08-31 時点、問題 13954 で実測)。

- **書き込みはすべて PUT です。** 問題・ジェネレータ・ジャッジコード・解説・想定解の保存は
  以前 POST でしたが、サーバから POST の経路は削除されています。失敗しても別のメソッドで
  送り直しません (原因の分からない失敗になるため)。
- サーバ側には送ったキーだけを更新する `PATCH /edit` もあります。本ツールの `push` は
  リポジトリの内容で全置換する使い方なので PUT を使っています。
- **問題文の `html` と `markdown` は排他。** サーバは `html` が非空ならそれをそのまま保存し、
  `markdown` は変換しません。両方送ると「表示は html・ソースは markdown」という食い違った状態になります。
  本ツールは Markdown なら `markdown` に本文・`html` は空文字列、HTML なら `html` に本文・`markdown` は送りません。
- **`POST /edit` は部分更新ではありません。** 省略した boolean は false になり、
  `html` と `markdown` を両方空にすると保存済みの問題文が消えます。
  設定は毎回すべて送り、本文が空なら POST 自体を中止します。
- 読み取り専用キー (`problemId` / `content` / `isMarkdown` / `showable`) は送りません。未知のフィールドはエラーになります。
- `latestModNanoTime` は WebUI の編集競合検出専用なので送りません (last-write-wins)。
- テストケースのアップロードは `POST /file/{in|out}` にフォーム名 `newfiles` (小文字) で
  複数まとめて送ります。ハンドラが読むフォーム名はこれだけです。
  1 件ずつ送る `POST /file/{in|out}/{FileName}` はルートが存在せず 404 になります
  (同じパスの GET / DELETE は使えます)。
- `eps` は API が文字列でも数値でも返すため、どちらも受け取って文字列として保持します。
- ジャッジコードの `GET /code` が 404 のサーバ (この API より前のバージョン) では、
  エラーにせず「未対応」として飛ばします。ほかの同期を巻き添えで止めないためです。

## ライセンス

Apache License 2.0 ([LICENSE](LICENSE))
