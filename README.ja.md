# truehdd
[![CI](https://github.com/truehdd/truehdd/workflows/CI/badge.svg)](https://github.com/truehdd/truehdd/actions/workflows/ci.yml)
[![Artifacts](https://github.com/truehdd/truehdd/workflows/Artifacts/badge.svg)](https://github.com/truehdd/truehdd/actions/workflows/release.yml)
[![Github all releases](https://img.shields.io/github/downloads/truehdd/truehdd/total.svg)](https://GitHub.com/truehdd/truehdd/releases/)

Dolby TrueHD ビットストリームをデコードするコマンドラインツールである。

**言語:** [English](README.md) | [简体中文](README.zh-CN.md) | 日本語

> ⚠️ **実験的ツール** 
> 
> 本ツールは研究・開発用途を目的としている。  
> 本番環境やエンドユーザーの再生システムでの使用は想定していない。
> 
> 💡 **新しいアイデアがあるか？**  
> 
> 追加すべき有用な機能があれば、issue や discussion を開いて知らせてほしい。

## 概要

`truehdd` は [truehd](truehd/) ライブラリの CLI frontend で、Dolby TrueHD 音声ビットストリームのデコード機能を提供する。

## インストール

### ソースからビルド

Rust 1.95.0 以降が必要である：

```bash
git clone https://github.com/truehdd/truehdd
cd truehdd
cargo build --release
```

コンパイルされたバイナリは `target/release/truehdd` にある。

## 使用方法

```
truehdd [オプション] <コマンド>

コマンド:
  decode    TrueHD ストリームを PCM 音声にデコード
  info      ストリーム情報を表示
  help      このメッセージまたは指定されたサブコマンドのヘルプを表示する

オプション:
      --loglevel <LOGLEVEL>         ログレベルを設定 [デフォルト: info]
                                    [可能な値: off, error, warn, info, debug, trace]
      --strict                      警告を致命的エラーとして扱う（最初の警告で終了）
      --log-format <LOG_FORMAT>     ログ出力形式 [デフォルト: plain]
                                    [可能な値: plain, json]
      --progress                    操作中に進捗バーを表示
  -h, --help                        ヘルプを表示する
  -V, --version                     バージョンを表示する
```

## コマンド

### `info` - ストリーム解析

TrueHD ストリームを解析し、デコードを行わずにその構造と特性に関する詳細な情報を表示する。

**使用法:** `truehdd info [オプション] <入力>`

```
引数:
  <入力>  入力 TrueHD ビットストリーム

オプション:
      --loglevel <LOGLEVEL>      ログレベルを設定 [デフォルト: info]
                                 [可能な値: off, error, warn, info, debug, trace]
      --strict                   警告を致命的エラーとして扱う（最初の警告で失敗）
      --log-format <LOG_FORMAT>  ログ出力形式 [デフォルト: plain] [可能な値: plain, json]
      --progress                 処理中に進捗バーを表示
  -h, --help                     ヘルプを表示
```

**使用例:**
```bash
# TrueHD ファイルを解析
truehdd info movie.thd
```

### `decode` - オーディオデコード

TrueHD ストリームを PCM 音声にデコードする。

**使用法:** `truehdd decode [オプション] <入力>`

```
引数:
  <入力>  入力 TrueHD ビットストリーム（標準入力には "-" を使用）

オプション:
      --output-path <PATH>       音声およびメタデータファイルの出力パス
      --format <FORMAT>          音声出力形式（プレゼンテーション3では常にCAFが使用される）
                                 [デフォルト: caf] [可能な値: caf, pcm, w64]
      --presentation <SELECTION> デコードするプレゼンテーション：インデックス (0-3)、リスト (0,1,3)、
                                 "all"、または利用可能な最上位を示す "max" [デフォルト: max]
      --no-estimate-progress     進捗推定を無効化
      --bed-conform              Atmosコンテンツのベッド適合を有効化
      --metadata-only            オブジェクトオーディオメタデータのみを出力し、PCM 出力を省略
      --json                     機械可読な結果サマリーを標準出力に出力
      --warp-mode <WARP_MODE>    メタデータにない場合のワープモードを指定
                                 [可能な値: normal, warping, prologiciix, loro]
      --probe-range <PROBE_RANGE>
                                 --bed-conform 使用時に Atmos メタデータを探索する
                                 アクセスユニットの上限 [デフォルト: 12000]
      --loglevel <LOGLEVEL>      ログレベルを設定 [デフォルト: info]
      --strict                   警告を致命的エラーとして扱う（最初の警告で失敗）
      --log-format <LOG_FORMAT>  ログ出力形式 [デフォルト: plain]
      --progress                 処理中に進捗バーを表示
  -h, --help                     ヘルプを表示
```

**出力ファイル:**

デフォルトでは、利用可能な最大のプレゼンテーションインデックスがデコードに選択される。
`--output-path` を指定すると、ツールは適切な出力ファイルを生成する：

- **チャンネルプレゼンテーション：** プレゼンテーションインデックス 0、1、または 2 の以下のファイルのいずれか
  - `output.caf` - Core Audio Format での PCM データ
  - `output.pcm` - Raw の PCM データ（`--format pcm` を使用した場合）
  - `output.wav` - Wave64 形式（`--format w64` を使用した場合）


- **オブジェクトプレゼンテーション：** プレゼンテーションインデックス 3 の Dolby Atmos マスターファイルセット（存在する場合）
  1. `output.atmos` - プレゼンテーションに関する基本情報
  2. `output.atmos.audio` - すべてのベッド信号とオブジェクトのオーディオ、Core Audio Format で
  3. `output.atmos.metadata` - 静的および動的信号の 3D 位置座標

  **注意：** プレゼンテーション3では `--format` オプションに関係なく常にCAF形式が使用される。`--bed-conform` を使用してベッドチャンネルを7.1.2レイアウトに変換する。


- **複数プレゼンテーション：** リストや `all`、複数のプレゼンテーションを選択した場合、実際に存在するプレゼンテーションが 1 つだけであっても、各出力ファイルにプレゼンテーションインデックスの接尾辞が付く（例：`output_p1.caf`、`output_p3.atmos`）。選択したプレゼンテーションは 1 回のデコードで処理され、サブストリームが共有する処理は再利用される。

**メタデータのみの出力：**

`--metadata-only` はオブジェクトプレゼンテーションの `.atmos` ヘッダーと `.atmos.metadata` のみを書き出し、音声ファイルを省略する。大量の PCM を生成せずにメタデータを確認・収集する場合に有用である。メタデータの内容は完全なデコードと同一である。`.atmos` ヘッダーには書き出さなかった音声ファイル名が残るため、得られるのはメタデータ一式であり、そのまま読み込めるマスターではない。オブジェクトオーディオメタデータを含まないプレゼンテーションでは何も書き出されない。

**機械可読な出力：**

`--json` を指定すると、デコード完了時に結果オブジェクトを標準出力に 1 つ出力する。呼び出し側はどのファイルが生成されたかを推測する必要がない：

```json
{
  "version": "0.5.0",
  "input": "movie.thd",
  "frames": 225526,
  "skippedFrames": 0,
  "branches": 0,
  "invalidBranches": 0,
  "samples": 9021040,
  "sampleRate": 48000,
  "presentations": [
    {"index": 3, "format": "damf", "channels": 12,
     "files": ["out.atmos", "out.atmos.audio", "out.atmos.metadata"]}
  ]
}
```

`channels` はチャンネル数が判明するまで `null` である。`skippedFrames` は抽出器が利用できず再同期によって飛ばしたフレーム数である。`branches` はデコーダのバッファモデルを満たすシームレスブランチ点の数、`invalidBranches` は満たさないものの数であり、後者は適合性の問題であってデコード結果のサンプルは変わらない。ログは標準エラーに出力されるため、標準出力にはこのオブジェクトのみが含まれる。ログも機械可読にする場合は `--log-format json` を使用する。

終了コードはどの段階で失敗したかを示す：

| コード | 意味 |
| --- | --- |
| 0 | 成功 |
| 1 | 分類されていない失敗 |
| 2 | コマンドラインが不正 |
| 3 | 入力を読み取れない |
| 4 | ビットストリームを解析できない |
| 5 | 音声をデコードできない |
| 6 | 出力を書き出せない |

`--strict` を使用した場合、スキップされたフレームも失敗として扱う。

**破損したストリーム：**

解析またはデコードに失敗したフレームは報告のうえスキップされ、次のメジャーシンクからデコードを再開する。中止はしない。最初の問題で失敗させる場合は `--strict` を使用する。

**ワープモードオプション:**

`--warp-mode` オプションは、メタデータにワープモード情報がない場合の Dolby Atmos コンテンツのダウンミックス処理方法を制御する：

- `normal` - 直接レンダリング
- `warping` - ルームバランス付きの直接レンダリング
- `prologiciix` - Dolby Pro Logic IIx
- `loro` - 標準（Lo/Ro）

このオプションは、元の OAMD メタデータにワープモード情報がない場合のみ適用される。メタデータに既にワープモードが含まれている場合、このオプションは無視される。

**使用例:**
```bash
# 進捗バー付きで TrueHD ファイルをデコード
truehdd decode --progress audio.thd --output-path decoded_audio

# メタデータにワープモード情報がないコンテンツに特定のワープモードを指定してデコード
truehdd decode --warp-mode prologiciix audio.thd --output-path decoded_audio

# 利用可能なすべてのプレゼンテーションを 1 回でデコード
truehdd decode --presentation all audio.thd --output-path decoded_audio

# ffmpeg パイプからデコード
ffmpeg -i movie.mkv -c copy -f truehd - | truehdd decode - --output-path audio
```

## ライセンス

Apache License 2.0 の下でライセンスされている。詳細は [LICENSE](LICENSE) を参照されたい。