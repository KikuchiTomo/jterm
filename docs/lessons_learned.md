# termojinal 修正履歴 — やらかしと教訓

このドキュメントは開発中に遭遇したバグと修正の記録。同じ間違いを繰り返さないための参照用。

---

## 1. Config パスが macOS で間違っていた

**症状**: config.toml を `~/.config/termojinal/` に置いても読み込まれない。フォントサイズや opacity が変わらない。

**原因**: `dirs::config_dir()` は macOS で `~/Library/Application Support/` を返す。`~/.config/termojinal/` ではない。

**修正**: XDG パス (`~/.config/termojinal/`) を優先し、なければ `dirs::config_dir()` にフォールバック。keybindings.toml も同様。

**教訓**: macOS では `dirs::config_dir()` ≠ `~/.config/`。XDG 準拠のパスを先に探すべき。

---

## 2. Config パースエラーが握りつぶされていた

**症状**: config.toml にタイポがあっても、エラーなしにデフォルト値が使われる。

**原因**: `toml::from_str(&content).unwrap_or_default()` がパースエラーを無視。

**修正**: `match` で分岐し、パースエラー時は `log::error!` で出力。

**教訓**: `unwrap_or_default()` はデバッグ困難な沈黙の失敗を引き起こす。config 読み込みでは必ずエラーをログに出す。

---

## 3. 背景透過が効かない — シェーダーが alpha=1.0 固定

**症状**: `opacity = 0.8` に設定しても背景が透過しない。ただしペイン間の隙間は透過する。

**原因**: シェーダーの `var alpha = 1.0;` が bg_color.a を無視していた。bg_opacity をセル背景に設定しても、シェーダーが常に alpha=1.0 を出力。

**修正**: `var alpha = in.bg_color.a;` に変更。グリフがある箇所は `alpha = 1.0` に強制（テキスト自体は透過しない）。

**教訓**: opacity の変更は「セルデータ → シェーダー → ブレンドモード → コンポジタ」の全パイプラインを通して追跡する必要がある。一箇所だけ変えても効かない。

---

## 4. 透過と不透明の二重適用 — clear + cell bg の alpha 蓄積

**症状**: opacity を設定してもほぼ不透明に見える。

**原因**: `clear_surface()` が alpha=bg_opacity でクリアし、その上にセルが alpha=bg_opacity で描画される。ALPHA_BLENDING で合成すると `output.a = 0.8 + 0.8 * 0.2 = 0.96`（ほぼ不透明）。

**修正**: `clear_surface()` は `(0,0,0,0)` でクリア。セルだけが alpha を持つ。

**教訓**: 透過レンダリングでは clear color と cell bg の alpha が二重に効かないよう注意。clear は完全透明にしてセルに任せる。

---

## 5. Content area 背景が opacity を無視 — 透過が突然効かなくなる

**症状**: ある変更後に opacity が完全に効かなくなった。

**原因**: `render_frame` で content area を `theme.background` (alpha=1.0) で塗りつぶしていた。opacity を `term_bg[3]` に設定し忘れ。

**修正**: `term_bg[3] = state.config.window.opacity` を追加。

**教訓**: 背景塗りつぶしを追加する際は、常に opacity が適用されているか確認する。opacity は壊れやすい。

---

## 6. Neovim 等の TUI アプリで背景が透過される

**症状**: neovim を起動しても背景が透過したまま。iTerm2 ではアプリの背景色が優先される。

**原因**: `bg == DEFAULT_BG` のチェックで、alternate screen のアプリが明示的に設定した背景色（たまたま DEFAULT_BG と同じ）も透過していた。

**修正**: `!terminal.modes.alternate_screen && bg == DEFAULT_BG` で alternate screen 時は透過を無効化。

**教訓**: TUI アプリ (alternate screen) では、ターミナルの透過設定よりアプリの背景色を優先すべき。

---

## 7. テキスト部分が透過する

**症状**: 透過有効時に文字の線自体が半透明になる。

**原因**: `alpha = max(alpha, glyph_alpha)` で glyph_alpha が 0.0〜1.0 の中間値（アンチエイリアス）の場合、alpha も中間値になる。

**修正**: `if glyph_alpha > 0.01 { alpha = 1.0; }` でグリフがある箇所は完全不透明に。

**教訓**: テキストのアンチエイリアスエッジで透過が発生しないよう、閾値で alpha を 1.0 に強制する。

---

## 8. DEFAULT_BG と theme.background の色の不一致

**症状**: ペインの余白部分（パディング）が微妙に違う色に見える。

**原因**: `color_convert::DEFAULT_BG` = `#111117` (ハードコード) と config の `theme.background` = `#11111A` が微妙に異なる。content area 背景は config の色、セル背景は DEFAULT_BG を使っていた。

**修正**: `renderer.default_bg` フィールドを追加し、config の `theme.background` から設定。セルのデフォルト背景もこの色を使用。

**教訓**: デフォルト色は一箇所で定義し、config の値で上書きする。ハードコード値と config 値の二重定義を避ける。

---

## 9. DPI スケーリング — scale_factor の扱い

**症状**: Retina ディスプレイで文字が小さすぎる / FHD に切り替えると文字が巨大になる。

**原因**: fontdue は物理ピクセル単位で rasterize する。config の `font.size` を論理ポイントとして扱い、`size * scale_factor` で物理ピクセルに変換する必要がある。

**修正**:
- `Renderer::new()` で `font_config.size * window.scale_factor()` でアトラス構築
- `set_font_size()` でも `size * self.scale_factor` で rasterize
- `ScaleFactorChanged` イベントで `renderer.scale_factor` を更新しアトラス再構築

**誤った修正の歴史**:
1. 最初: scale_factor を掛けなかった → Retina で文字が小さい
2. scale_factor を掛けた → FHD で文字が大きい（ユーザーが size=22 というワークアラウンドを使っていたため）
3. scale_factor を外した → Retina でまた小さい
4. 最終: scale_factor を掛ける + config の size はデフォルト 14pt（論理ポイント）

**教訓**: config の font.size は論理ポイント。fontdue には `size * scale_factor` を渡す。ディスプレイ切り替え時はアトラス再構築が必要。

---

## 10. CJK/日本語の半幅表示

**症状**: 日本語文字が半分の幅で表示される。

**原因**:
- **ターミナルグリッド**: `cell.width = 2` + `cell_width_scale` で正しく処理済み
- **preedit (IME入力中)**: `cell_width_scale: 1.0` がハードコードされていた
- **UI テキスト (render_text)**: 同様に `cell_width_scale: 1.0` 固定

**修正**: preedit と render_text の両方で `unicode_width` に基づいて `cell_width_scale` を設定。

**教訓**: CJK 幅の処理は「ターミナルグリッド」「preedit」「UI テキスト」の 3 箇所すべてで必要。

---

## 11. 罫線 (box-drawing) のずれ

**症状**: lazygit 等で縦線・横線の結合がずれる。

**原因**: U+2500〜U+257F の box-drawing 文字がフォントグリフから描画されていた。フォントのメトリクスがセル境界と一致しないため隙間が生じる。

**修正**: box-drawing 文字をプロシージャル描画に変更。セル端まで正確に線を引くことで隣接セルと隙間なく接続。

**教訓**: ブロック要素だけでなく box-drawing 文字もプロシージャル描画すべき。フォントグリフはセル境界の保証がない。

---

## 12. ステータスバーのテキストがずれる

**症状**: セグメント背景の中でテキストが左寄り、上寄り。

**原因 (複数)**:
1. **幅計算**: `expanded.len()` (バイト数) × cell_w で計算 → Unicode 文字で幅がずれる
2. **センタリング無効**: セグメント幅 = テキスト幅で、`(seg_w - text_w)/2 = 0` → センタリングが効かない
3. **垂直位置**: `(bar_h - cell_h)/2` の数学的中央はデセンダー領域のため視覚的に上寄り
4. **content 文字列に手動スペース**: アイコン+スペースが非対称パディングを作る
5. **scissor rect**: `cell_h` でクリップされてバー高さと合わない

**修正**:
1. `str_display_width()` (unicode-width) で正しい表示幅を計算
2. セグメント幅 = テキスト幅 + `cell_w * 2` (左右パディング)。テキストはその中で水平中央
3. 光学的中央揃え: `descent * 0.4` 分下にオフセット
4. content からスペース/アイコンを除去し、コード側でパディング+センタリング
5. `render_text_clipped` でセグメント全体の rect を scissor に指定

**教訓**:
- セグメント幅 ≠ テキスト幅にしないとセンタリングが効かない
- 文字列のバイト長 `.len()` と表示幅は異なる（Unicode）
- テキストの視覚的中央 ≠ 数学的中央（デセンダーを考慮）
- パディングはコンテンツ文字列ではなくレンダリングコードで管理すべき

---

## 13. render_text に透明 bg を渡すとガビガビになる

**症状**: ステータスバーのテキストが汚く表示される。

**原因**: `bg = [0,0,0,0]` (透明) を render_text に渡すと、シェーダーの `mix(bg.rgb, fg.rgb, glyph_alpha)` でアンチエイリアスエッジが黒とブレンドされる。

**修正**: 実際のセグメント背景色を bg として渡す。

**教訓**: render_text の bg は「アンチエイリアスのブレンド先」として使われる。透明にすると文字が汚くなる。常に実際の背景色を渡す。

---

## 一般的な教訓まとめ

1. **opacity は壊れやすい**: 背景塗りつぶし、シェーダー出力、clear color、content area fill — どれか一つでも alpha=1.0 だと透過が効かなくなる
2. **座標系を統一する**: `submit_separator` (u32) と `render_text` (f32) の座標が一致しないと 1px ずれる
3. **config のデフォルト値とハードコード値を二重定義しない**: 色は一箇所で管理
4. **Unicode 幅は `.len()` ではない**: `unicode_width::UnicodeWidthChar` を使う
5. **macOS の DPI**: font.size は論理ポイント。fontdue には `* scale_factor` を渡す
6. **alternate screen では透過を切る**: TUI アプリの背景を尊重
7. **パディングはレンダリングコードで管理**: content 文字列にスペースを入れてパディングするとセンタリングが破綻する
8. **視覚的中央 ≠ 数学的中央**: デセンダーを考慮した光学的中央揃えが必要
