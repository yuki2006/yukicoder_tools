# GitHub Actions で問題を更新する

リポジトリを正として、main に入った内容を yukicoder へ反映するまでの手順です。
問題 13954 を例にします。

## 1. 準備

### トークンを Secrets に登録する

問題の管理画面で編集トークン (`ypt_...`) を発行し、リポジトリの
**Settings → Secrets and variables → Actions → New repository secret** に登録します。

| Name | Secret |
| --- | --- |
| `YUKICODER_TOKEN` | `ypt_...` |

複数の問題を扱うなら、問題ごとに `YUKICODER_TOKEN_13954` のような名前で登録し、
workflow の `env` にそのまま渡します。アカウントの API キーを使うなら `YUKICODER_API_KEY` です。

> トークンをリポジトリに直接書かないでください。ローカルでは `.env` を使います (`.gitignore` 済み)。

### PR を作る workflow を使うなら

`problem-pull.yml` は PR を作るので、**Settings → Actions → General → Workflow permissions** で
「Allow GitHub Actions to create and approve pull requests」を有効にします。

### 問題をリポジトリに取り込む

最初の 1 回だけローカルで実行します。

```sh
cargo run -- init 13954
```

`yukicoder.toml` と `problems/13954/` ができます。これをコミットすれば準備完了です。

ディレクトリ名は後から自由に変えられます。どの問題かは `problem.toml` の `problemId` で
決まるので、`problems/tutorial-dp/` や `problems/abc001/a/` のような置き方もできます。

2 問目からは `yuki new` を使います。yukicoder で問題を作って ID とトークンを発行したら、

```sh
cargo run -- new 20000 --dir abc001/a
```

で `problems/abc001/a/` に問題一式 (設定・問題文テンプレート・testcases/ solutions/ の
骨組み) ができます。

## 2. 更新する

### 問題文や設定を変える

`problems/13954/statement.md` や `problems/13954/problem.toml` を編集して PR を出します。

```sh
git switch -c fix-statement
# statement.md を編集
git commit -am "問題文の入力の説明を直す"
git push -u origin fix-statement
```

### PR で差分を確認する

`problem-diff.yml` が動き、**yukicoder の現在の内容とこの PR の内容の差分**が
ジョブサマリに出ます。実際の出力はこの形です。

```diff
--- 問題 13954 ---
問題設定: 差分なし
問題文:
--- yukicoder
+++ ローカル
@@ -8,7 +8,7 @@
 ## @input 入力

-<!-- ここに入力の説明、制約等を記述してください。 -->
+$1 \le N \le 10^5$

テストケース in: 差分なし (0 件)
テストケース out: 差分なし (0 件)
```

差分があること自体は失敗になりません (PR は「これから反映する変更」なので)。
落ちるのは `problem.toml` が壊れているなど、`yuki` 自体がエラーになった場合だけです。

### マージすると反映される

main にマージされると `problem-push.yml` が動き、yukicoder へ反映されます。
ジョブサマリに結果が出ます。

```
問題 13954 を反映しています (problems/13954)
  問題: 保存されました。
  テストケース in: 差分なし (0 件)
  テストケース out: 差分なし (0 件)
```

反映は last-write-wins です。**WebUI 側で直接編集していた内容はここで上書きされます。**

## 3. 手動で実行する

`problem-push.yml` は **Actions → 問題を yukicoder に反映 → Run workflow** から手動でも動かせます。
2 つの入力があります。

- **dry_run** — 送信内容を表示するだけで反映しない。設定を変えたときの確認に使う
- **prune** — ローカルに無いテストケースを yukicoder から削除する。既定は off

テストケースの削除を既定にしていないのは、ジャッジデータを事故で消さないためです。

## 4. WebUI で編集してしまったら

**Actions → yukicoder の変更を取り込む → Run workflow** を実行してください。
yukicoder 側にあってリポジトリに無い変更を取り込む PR ができます。定期実行はしません。

この PR をマージしてからリポジトリ側の編集を進めてください。ずれたまま main に何か入ると、
`problem-push.yml` が WebUI 側の編集を上書きします。

## 5. スペシャルジャッジを更新する

`problems/13954/judge/` にジャッジコードを置きます。

```toml
# problems/13954/judge/judge.toml
langId = "cpp17"
sourceFile = "judge.cpp"
```

`problem.toml` の `judgeType` を 1 (スペシャル) 以上にしてください。0 のままだと保存はできても
使われません (`push` が警告を出します)。同じコミットで両方を変えれば 1 回で反映されます。

`push` は保存後にコンパイルの完了を待ち、`CE` なら**終了コード 1 で失敗します**。
コンパイルの通らないジャッジコードを CI が黙って通さないためです。

```
  ジャッジコード: 保存しました。コンパイルを開始します。
  ジャッジコード: コンパイル成功 (AC)
```

コンパイルを待たずに進めたいときは、workflow の実行コマンドに `--no-wait-compile` を足します。

## 6. 想定解を登録する

提出と想定解の登録は CI に載せず、手元から実行することを想定しています。
提出は結果を人が確認するものだからです。

```sh
cargo run -- submit --file problems/13954/solutions/main.cpp --lang cpp23
# 提出しました: 提出 ID 1181846
# AC を確認してから
cargo run -- solution 1181846 --summary "想定解 O(N log N)"
```

## つまずきやすいところ

| 症状 | 原因 |
| --- | --- |
| `トークンが見つかりません` | Secrets が job の `env` に渡っていない。`env:` の位置を確認する |
| `HTTP 401` | トークンが別の問題のもの、または失効している |
| `HTTP 403` | 編集権限が無い、または入力の検証エラー。本文にメッセージが出る |
| `HTTP 500` | サーバ側のエラー。保存や削除は行われていない。同じコマンドを実行し直す |
| フォークからの PR で diff が動かない | 想定どおり。フォークには Secrets が渡らない |
| 毎回テストケースに差分が出る | 改行が CRLF になっている。`.gitattributes` の `eol=lf` を確認する |
