# ImportLint

[![crates.io](https://img.shields.io/crates/v/import-lint.svg)](https://crates.io/crates/import-lint)
[![npm](https://img.shields.io/npm/v/%40import-lint%2Fcli.svg)](https://www.npmjs.com/package/@import-lint/cli)

[English](./README.md) | 日本語

**ImportLint** は、TypeScript・JavaScript にディレクトリ単位のカプセル化をもたらす lint ツールです。ディレクトリを「パッケージ」とみなし、そのエクスポートはJSDoc で `@public` とタグ付けしない限り、そのディレクトリの内側(および配下)のファイルからしかインポートできません。このルールに違反するインポートをImportLint がすべて検出します。Rust 製の小さな CLI なので、大規模なコードベースでも高速に動作します。

**`eslint-plugin-import-access` を使用中ですか？**[移行ガイド](#eslint-plugin-import-access-からの移行)をご覧ください。

## Why?

TypeScript や JavaScript が提供してくれるカプセル化の最大単位はファイルです。export しないものはファイルの外からは見えませんが、export した瞬間にプロジェクトのどこからでも見えて、インポートできるようになります。「この5つのファイルの間だけで共有したいが、コードベースの残りには見せたくない」を表現する組み込みの手段はありません。

ImportLint はファイルの上にディレクトリというレイヤーを追加します。各ディレクトリ（または[`packageDirectory`](#設定ファイル)で指定した境界）が「パッケージ」となり、そのエクスポートは `@public` とタグ付けしない限りパッケージの内側からしかインポートできません。

## 例

```
src/
├── cart/
│   └── total.ts     ── computeTotal()
└── receipt.ts        ── cart/ の外から computeTotal をインポート
```

`.importlintrc.jsonc`:

```jsonc
{
  "rules": {
    "package-access": {
      "defaultImportability": "package"
    }
  }
}
```

`src/cart/total.ts`:

```ts
export function computeTotal(items: number[]): number {
  return items.reduce((a, b) => a + b, 0);
}
```

`src/receipt.ts`:

```ts
import { computeTotal } from "./cart/total";

console.log(computeTotal([1, 2, 3]));
```

```
$ import-lint .
src/receipt.ts
  1:10  error  Cannot import a package-private export 'computeTotal'  package-access

✖ 1 problem (1 error, 0 warnings)
```

修正は簡単です。`computeTotal` をどこからでも使えるようにしたいなら`/** @public */` タグを付けましょう。あるいは、`receipt.ts` を `cart/` の中に移動します。

既存プロジェクトに段階的に導入することも可能です。デフォルトの設定では、`@package`タグを付けたエクスポートのみが制限の対象となります。

全体的なメンタルモデルは[Concepts ガイド](https://github.com/uhyo/import-lint/blob/master/docs/guides/concepts.md)を参照してください。より長い[チュートリアル](https://github.com/uhyo/import-lint/blob/master/docs/guides/tutorial.md)もあります。

## プロジェクトの状態

v1.0.0 に到達するまでは**ベータ版**です。本プロジェクトは完全にVibe Codingで開発されていますが、すでにプロダクションでの有用性が実証された`eslint-plugin-import-access`と全く同じように動作するように作られています。そして、すでにESLint プラグイン版の100倍高速です。

## はじめに

### インストール

**npm**:

```sh
npm install -D @import-lint/cli
```

これで `import-lint` コマンドがインストールされ、`npx import-lint`で実行可能になります。

**Cargo**:

```sh
cargo install import-lint
```

あるいは、[GitHub Releases](https://github.com/uhyo/import-lint/releases) からお使いのプラットフォーム向けのビルド済みバイナリを入手することもできます。

### 設定と実行

```sh
# npm でインストールした場合は npx を付けてください(例: `npx import-lint`)。

# .importlintrc.jsonc を生成
import-lint init

# 設定ファイルに従ってlintを実行
import-lint

# 特定のパスを lint (設定の `include` を上書き)
import-lint src lib

# CI ツール向けの ESLint 互換 JSON 出力
import-lint --format json
```

`import-lint init` は、上で示した推奨の「デフォルトでパッケージプライベート」設定で、コメントが充実した `.importlintrc.jsonc` を生成します。設定ファイルがない場合はデフォルト設定が使われます。詳細は後述の [設定ファイル](#設定ファイル) と、展開の仕方を解説した[Adoption ガイド](https://github.com/uhyo/import-lint/blob/master/docs/guides/adoption.md)を参照してください。

### ガイド

[`docs/guides/`](https://github.com/uhyo/import-lint/tree/master/docs/guides)に3つの短いガイドが用意されています。

- [**Concepts**](https://github.com/uhyo/import-lint/blob/master/docs/guides/concepts.md) — メンタルモデル: importability、パッケージディレクトリ、2つのループホール、one-hop 再エクスポートのセマンティクス、external と internal の区別
- [**Tutorial**](https://github.com/uhyo/import-lint/blob/master/docs/guides/tutorial.md) — 10分ほどのウォークスルー。境界を作り、違反を起こし、3通りの方法で修正します。
- [**Adoption**](https://github.com/uhyo/import-lint/blob/master/docs/guides/adoption.md) — 初期設定の選択と展開。既存コードベースへの段階的な導入戦略も含みます。

## CLI フラグ

```
import-lint [paths...]
```

| フラグ | 説明 | デフォルト |
|---|---|---|
| `paths...` | lint 対象のパス。指定すると設定ファイルの `include` を上書きします | 設定の `include`、設定がなければ `.` |
| `--config <path>` | 設定ファイルの明示的な指定。存在しないか不正な場合は終了コード `2` になります | カレントディレクトリから上に向かって探索 |
| `--format <pretty\|json\|github>` | 出力フォーマット — [出力フォーマット](#出力フォーマット) を参照 | `pretty` |
| `--threads <n>` | 使用するスレッドの数 | コア数 |
| `--tsconfig <path>` | リゾルバの `paths`/`baseUrl` に使うプロジェクトの `tsconfig.json` のパス | 設定の `tsconfig`、なければ `<プロジェクトルート>/tsconfig.json`(存在する場合) |
| `--report-unresolved` | 解決に失敗した import 指定子を黙ってスキップせず、1件ずつ警告として報告する | オフ |
| `--quiet` | ワーニングの出力を抑制(エラーのみ表示)。`eslint --quiet` と同様 | オフ |
| `--watch` | [watch モード](#watch-モード) | オフ |
| `--watch-poll [ms]` | ポーリング方式のウォッチャーを使う watch モード。`--watch` を含意します | オフ |

`import-lint init [--force]` はカレントディレクトリに`.importlintrc.jsonc` を生成します。このほかに2つのデバッグ用サブコマンドがあります(セマンティックバージョニングの対象外です)。`import-lint inspect <file>` は1ファイルから抽出したモジュール情報を JSON で出力し、`import-lint graph [paths...]` は探索・解決グラフを JSON で出力します。

フラグの優先順位は **CLI フラグ > 設定ファイル > 組み込みデフォルト** です。ルールオプション(`indexLoophole`、`defaultImportability` など)は設定ファイルからのみ設定できます。

## 設定ファイル

設定ファイルは手書きせずとも `import-lint init` で生成できます。生成される設定は推奨の「デフォルトでパッケージプライベート」設定です: エクスポートは `@public` タグを付けない限りパッケージプライベートになり、`foo.package` という名前のディレクトリがカプセル化の境界になります。リネームしていないディレクトリのファイルはすべてプロジェクトルート直下の1つのパッケージに属し、互いに自由にインポートできるため、既存コードベースへの段階的な導入にもそのまま使えます — ディレクトリを1つずつリネームしていくだけです。ほかの構成(アノテーション駆動の導入や、モノレポの `packages/*` を境界にする構成)も設定を少し編集するだけで実現できます。[Adoption ガイド](https://github.com/uhyo/import-lint/blob/master/docs/guides/adoption.md)で解説しています。

ImportLint は、`--config` で明示的にファイルを指定しない限り、カレントディレクトリからファイルシステムのルートまで上に向かって`.importlintrc.jsonc`(同じディレクトリに `.jsonc` ファイルがない場合は`.importlintrc.json`)を探します。**設定ファイルのあるディレクトリがプロジェクトルートになります**。 `include`、`exclude`、`tsconfig` はすべてそこからの相対パスとして解決されます。設定ファイルが見つからない場合は、カレントディレクトリをプロジェクトルートとして、以下のデフォルトが使われます。

以下は全オプションのクイックリファレンスです(組み込みデフォルト値を併記)。[Concepts ガイド](https://github.com/uhyo/import-lint/blob/master/docs/guides/concepts.md)では各オプションを具体例つきで説明しています。

```jsonc
// .importlintrc.jsonc
{
  // lint 対象を探索するルート。プロジェクトルートからの相対パス。
  "include": ["."],

  // .gitignore に加えてスキップする glob パターン。プロジェクトルートからの相対パス。
  "exclude": [],

  // リゾルバの `paths`/`baseUrl` に使う tsconfig.json のパス。プロジェクト
  // ルートからの相対パス。存在すれば "<プロジェクトルート>/tsconfig.json" がデフォルト。
  // "tsconfig": "./tsconfig.json",

  "rules": {
    "package-access": {
      // "error" | "warn" | "off"。`off` にしたルールは一切チェックされません。
      "severity": "error",

      // 以下は eslint-plugin-import-access の `import-access/jsdoc` ルールと
      // 同一のオプション・名前・デフォルト値です。

      // "index.{js,ts,jsx,tsx,mjs,cjs,...}" という名前のファイルを、パッケージ
      // 境界の判定においては親ディレクトリに属しているものとして扱います。
      "indexLoophole": true,

      // "foo/bar.ts" を "foo.ts" と同一パッケージとして扱います(インポート元の
      // ファイル名(拡張子を除く)に一致するディレクトリ1階層分)。
      "filenameLoophole": false,

      // 認識可能な JSDoc アクセスタグのないエクスポートに適用されるアクセスレベル。
      // "public" | "package" | "private"。組み込みデフォルトは "public"
      // ですが、推奨は "package" で、`import-lint init` が生成する値です。
      "defaultImportability": "public",

      // インポート元自身のパッケージ名に一致する bare specifier の扱い。
      // "external"(チェックしない)| "internal"(通常どおりチェック)。
      "treatSelfReferenceAs": "external",

      // チェック対象から常に除外する glob パターン(エクスポート元ファイルの
      // プロジェクト相対パスに対してマッチ)。アクセスレベルに関係なく適用されます。
      "excludeSourcePatterns": [],

      // 「パッケージ」ディレクトリを識別する glob パターン。
      // 未設定の場合、ファイルの属するディレクトリがそのままそのファイルの
      // パッケージになります。設定した場合、マッチする祖先ディレクトリを
      // 持たないファイルは、プロジェクトルートの単一パッケージに属します。
      // `!` 付きのパターンで、本来マッチするディレクトリを除外できます。
      // "packageDirectory": ["packages/*"],
    }
  }
}
```

設定ファイル内の未知のキー(オプション名の typo や、認識されないルール)は、黙って無視されるのではなく、ロードエラー(終了コード `2`)になります。

## 出力フォーマット

- **`pretty`**(デフォルト)— ESLint の stylish に似た、ファイルごとにグループ化された形式。パスはカレントディレクトリからの相対パスです。stdout がTTY のときは色付き、それ以外はプレーンな出力になります。クリーンな実行では何も出力しません。

  ```
  src/foo/bar.ts
    3:10  error  Cannot import a package-private export 'x'  package-access
    5:10  warning  Unresolved import specifier './gone'  import-access/unresolved

  ✖ 2 problems (1 error, 1 warning)
  ```

- **`json`** — 1行の ESLint 互換 JSON 配列。lint した各ファイルにつき1エントリで、問題のないファイルも `messages` が空配列のエントリとして含まれます(ESLint 自身の挙動に合わせています)。各エントリは `filePath`、`messages`(`ruleId`、`severity`(`2` = error、`1` = warning)、`message`、`messageId`、`line`、`column`、`endLine`、`endColumn`)、`errorCount`、`warningCount`、および `fixableErrorCount` / `fixableWarningCount`(常に `0` — ImportLint にautofix はありません)を持ちます。`eslint --format json` を読み込む既存のツールにそのまま使えます。

- **`github`** — 診断1件につき1行の[GitHub Actions ワークフローコマンド](https://docs.github.com/en/actions/using-workflows/workflow-commands-for-github-actions)を出力します:

  ```
  ::error file=src/a.ts,line=3,col=10,endLine=3,endColumn=20::Cannot import a package-private export 'x'
  ```

## CI での利用

エラーレベルの診断が1件でもあれば終了コード `1` になるため、ImportLint はそのまま CI で使えます。また `--format github` は GitHub Actions のワークフローコマンドを出力するので、違反が PR 上のインラインアノテーションとして表示されます。

```yaml
jobs:
  import-lint:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with:
          node-version: 20
      - run: npm ci
      - run: npx import-lint --format github
```

機械可読な出力が必要な場合(他のツールに渡すなど)は、ESLint 互換の`--format json` を使ってください。

## 終了コード

| コード | 意味 |
|---|---|
| `0` | エラーレベルの診断なし(クリーンな実行、または警告のみ)。 |
| `1` | エラーレベルの診断が1件以上。 |
| `2` | 不正な使い方、`--config` ファイルが不正または存在しない、あるいは内部エラー。 |

`--report-unresolved` による診断と、`"severity": "warn"` に設定されたルールの診断は警告です — 出力には含まれますが(`--quiet` 時を除く)、終了コードには影響しません。

## watch モード

```sh
import-lint --watch
```

最初に一度 lint を実行し、その後はプロセスを終了する(Ctrl-C)まで、ファイルの変更のたびに再 lint し続けます。各サイクルで診断リスト全体を出力し直します — TTY でデフォルトの `pretty` フォーマットの場合は、先に画面をクリアします。出力をパイプ・リダイレクトしている場合(または `--format json` / `--format github`)は各サイクルの全出力を追記していくだけなので、`import-lint --watch --format json | tee log.jsonl` で読みやすいログが得られます。再描画のたびにステータス行が続きます:

```
✖ 1 problem (1 error, 0 warnings) — rechecked 42 files in 8 ms (watching, Ctrl-C to exit)
```

**`--watch-poll [interval-ms]`**(デフォルト間隔 `500`)は、プラットフォーム推奨のウォッチャー(Linux では inotify)の代わりにポーリング方式のウォッチャーを使います。次の場合に使ってください:

- **WSL2 で、Windows 側からファイルを編集している場合**(例: Windows 上で動くVS Code から `\\wsl$\...` や `/mnt/c/...` のパスを開いている場合)— inotify はLinux VM の外で発生した書き込みを確実には検知できません(`docs/research/spike-s5-watch-wsl2.md` を参照)。
- **ネットワークファイルシステム上**(NFS、Samba など)— inotify のサポートは一般に不安定か、存在しません。

**制限事項:** `node_modules` は決して watch されません(探索が `node_modules`の中に入らないのと同様です)— `node_modules` の変更が再実行をトリガーすることはありません。依存関係を再インストールした場合や、`node_modules` 配下のリンク済みパッケージ・ワークスペースパッケージを編集した場合は、`import-lint --watch` を手動で再起動してください。

## エディタ統合

**[ImportLint VS Code 拡張機能](https://marketplace.visualstudio.com/items?itemName=uhyo.import-lint)**(`uhyo.import-lint`、[Open VSX](https://open-vsx.org/extension/uhyo/import-lint)にもあります)を使うと、入力中に違反が表示されます。`npm install -D @import-lint/cli`以外の追加インストールは不要です: 拡張機能はワークスペース内のバイナリを自動で見つけます(`importLint.binaryPath` で上書きでき、フォールバックは `PATH` です)。`.importlintrc.json(c)` が存在すると自動的に有効になり(`importLint.enabled` で強制的にオン・オフできます)、`importLint.run` で、毎キーストロークで lint する(`onType`、デフォルト)か保存時のみにする(`onSave`)かを選べます。

**その他のエディタ:** 同じバイナリが `import-lint lsp` で、stdio 越しに[LSP](https://microsoft.github.io/language-server-protocol/) を話します。Neovim(0.11+、`vim.lsp.config`/`vim.lsp.enable` を使用)の場合:

```lua
vim.lsp.config('import_lint', {
  cmd = { 'import-lint', 'lsp' },
  filetypes = { 'javascript', 'javascriptreact', 'typescript', 'typescriptreact' },
  root_markers = { '.importlintrc.jsonc', '.importlintrc.json', '.git' },
})
vim.lsp.enable('import_lint')
```

## eslint-plugin-import-access からの移行

ImportLint の `package-access` ルールはESLintプラグインの `import-access/jsdoc`ルールの挙動を移植したものです。以下の手順で移行できます。

- **パッケージを入れ替える**: `package.json` の `devDependencies` で`eslint-plugin-import-access` を `@import-lint/cli` に置き換えます(`npm uninstall eslint-plugin-import-access && npm install -D @import-lint/cli`)。インストールされるコマンドは `import-lint` です。ESLint 設定からはプラグインを削除してください。
- **オプションは同じ名前で1対1に対応**し、デフォルト値も同じです。ルールオプションをそのまま `.importlintrc.jsonc` の `rules.package-access` にコピーしてください(ESLint プラグインの `import-access/jsdoc` というルール名は`rules.package-access` になりました)。
- `json` 出力フォーマットの `ruleId` は `package-access` になっています（`import-access/jsdoc` ではありません）。ESLint プラグインのルール ID を前提にしている CI のフィルタや `reviewdog` のルール ID マッチは更新してください。

ただし、1つだけ**挙動の変更**があります。`packageDirectory` を設定した場合、マッチする祖先ディレクトリを*持たない*ファイルはプロジェクトルートのパッケージに属し、自由にインポートし合うことができます。ESLint版では、この場合ファイルが属するディレクトリがパッケージとして扱われてしまっていました。

## パフォーマンス

16コアの AMD Ryzen 7 PRO 6850U ラップトップ上の **WSL2** で計測しています。詳細は `docs/benchmarks.md` を参照してください。

同じ5,000ファイルのツリーに対して、本ツールはリファレンス実装である**`eslint-plugin-import-access` の約155倍高速**です(157 ms 対 24.4 秒)。これは、ESLint版がtypescript-eslintの型情報を用いていたのに対し、ImportLintではoxcでパースし、型情報を使用していないためです。

再現するには `scripts/bench.sh`(ESLint との比較には `--compare-eslint` を追加)と `cargo bench -p import-lint-core --bench extract` を実行してください。
