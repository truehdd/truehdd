# truehdd

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

Rust 1.87.0 以降が必要である：

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
...
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
      --format <FORMAT>          音声出力形式 [デフォルト: caf] [可能な値: caf, pcm]
      --presentation <INDEX>     presentation インデックス (0-3) [デフォルト: 3]
      --no-estimate-progress     進捗推定を無効化   
...
```

**出力ファイル:**

`--output-path` を指定すると、ツールは適切な出力ファイルを生成する：

*通常の TrueHD ストリーム:*
- `output.caf` - Core Audio Format
- `output.pcm` - Raw の 24 bit PCM（`--format pcm` を使用した場合）

*Dolby Atmos ストリーム:*
- `output.atmos` - DAMF header ファイル
- `output.atmos.audio` - CAF 形式の audio data
- `output.atmos.metadata` - DAMF metadata ファイル

**使用例:**
```bash
# 進捗バー付きで TrueHD ファイルをデコードし、出力なし
truehdd decode --progress audio.thd --output-path decoded_audio

# ffmpeg パイプからデコード
ffmpeg -i movie.mkv -c copy -f truehd - | truehdd decode - --output-path audio
```

## ライセンス

Apache License 2.0 の下でライセンスされている。詳細は [LICENSE](LICENSE) を参照されたい。