# Gooey — Agent Instructions

Gooey は **5ch ブラウザ** の GUI アプリ。Rust + [gpui](https://github.com/zed-industries/zed/tree/main/crates/gpui) (GPU 加速 GUI) + [gpui-component](https://github.com/longbridge/gpui-component) コンポーネントで構成。

## Build & Run

```bash
cargo build           # デバッグビルド
cargo build --release # リリースビルド
cargo run             # 実行（デバッグ）
cargo run --release   # 実行（リリース）
```

テストコードは現状なし。ビルドが通れば OK。

## Architecture

全ロジックは **`src/main.rs` 1 ファイル**に集約されている（意図的な設計）。モジュール分割はしない。

| 領域 | 内容 |
|------|------|
| データ取得 | `ureq` で同期 HTTP → `cx.background_executor().spawn()` でバックグラウンド実行 |
| 文字コード | 5ch は Shift_JIS — `encoding_rs` でデコード |
| キャッシュ | `%LOCALAPPDATA%\gooey\` 以下にファイルキャッシュ。存在すればネットワーク取得をスキップ |
| セッション | `session.json` に JSON 保存 — 状態変更ごとに `save_session_to_disk()` を呼ぶ |
| テーマ | `themes/gooey-custom-themes.json` — `ThemeRegistry::watch_dir` でホットリロード |
| 仮想リスト | `v_virtual_list` でスレッド一覧・レス表示を仮想スクロール |

## gpui / gpui-component の重要な注意点

- **初期化**: `app.run` 冒頭で `gpui_component::init(cx)` が必須
- **`cx.new()`**: `use gpui::AppContext as _` の import が必要
- **アイコン**: `icons/*.svg` は `Application::with_assets(AppAssets)` で `AssetSource` を登録しないと表示されない
- **バックグラウンド取得**:
  ```rust
  cx.spawn(async move |_weak_self, cx| {
      cx.background_executor().spawn(async move { blocking_fn() }).await;
      entity.update(cx, |state, cx| { ... });
  })
  ```
- **`TreeState`**: `selected_item()` は存在しない → `selected_entry().item().label` を使う
- **`resizable`**: `h_resizable / v_resizable` + `resizable_panel().size().size_range()` の組み合わせ
- **色**: `rgb(...)` は `Rgba` 型を返す

## Conventions

- コメントは**日本語**で書く
- テーマカラーは `Theme::global(cx)` から取得しレンダリングクロージャにキャプチャして使う
- `px(16. * depth as f32)` のように `usize` → `f32` 変換して `px()` に渡す
