# Fairy Comic Library

漫画アーカイブを整理・検索する、Ubuntu 向けのデスクトップアプリです。  

## 主な機能

- ZIP / CBZ、7z、RAR / CBR のインポートとライブラリ管理
- SHA-256 による重複検出と、アーカイブからの表紙サムネイル生成
- お気に入り、作者・シリーズ・分類タグ、検索、絞り込み、並べ替え
- アーカイブを外部ビューアーで開き、読書履歴を記録
- リポジトリの検証、サムネイルの再作成、別の PC へ移行できるバックアップ
- ZIP / CBZ を 7z + AVIF に変換（非可逆変換。実行前に確認が表示されます）

## 動作環境

- Ubuntu x86_64
- Node.js 24 以降
- Rust stable 1.85 以降
- AppImage の実行には FUSE 2 互換ライブラリが必要です。Ubuntu 24.04 / 26.04 では `libfuse2t64` を使用してください（26.04 の標準 FUSE 3 だけでは足りません）。

開発環境のセットアップに必要な Linux システムパッケージは、[Tauri の Linux 前提条件](https://v2.tauri.app/start/prerequisites/#linux)を参照してください。

## 使い方

1. 配布された AppImage を実行します。
2. アプリ管理リポジトリの場所を選択するか、新しい空のディレクトリを選択します。
3. 「＋」ボタンまたはドラッグ＆ドロップでアーカイブを追加します。

アーカイブは選択したリポジトリへ移動します。閲覧するには、同梱の Brave Comic Reader が起動します。

## 開発

依存パッケージをインストールし、開発モードで起動します。

```bash
npm ci
npm run tauri dev
```

フロントエンドの型チェックとビルドは次のコマンドで実行できます。

```bash
npm run build
```

## リリースビルド

```bash
npm run build:release
```

このコマンドは [Brave Comic Reader](https://github.com/takanemu/braveComicReader) の最新リリースから Linux x86_64 用ビューアーを取得し、チェックサムを検証してから AppImage をビルドします。GitHub へのネットワーク接続が必要です。

生成物は `src-tauri/target/release/bundle/appimage/` に出力されます。

## アプリ管理リポジトリ

選択したディレクトリに、アーカイブ・サムネイル・SQLite データベースをまとめて保存します。バックアップもリポジトリ全体を対象とするため、別の PC ではコピー先を「場所を変更」から選び直して利用できます。

```text
<repository>/
├── repository.json
├── archives/
│   └── <ハッシュ先頭2文字>/
│       └── <元のファイル名>
├── thumbnails/
│   └── <ハッシュ先頭2文字>/
│       └── <SHA-256>.webp
└── database/
    └── library.sqlite3
```

## アイコンの生成

```bash
npm run tauri icon app-icon-tp.png
```
