# truehdd
[![CI](https://github.com/truehdd/truehdd/workflows/CI/badge.svg)](https://github.com/truehdd/truehdd/actions/workflows/ci.yml)
[![Artifacts](https://github.com/truehdd/truehdd/workflows/Artifacts/badge.svg)](https://github.com/truehdd/truehdd/actions/workflows/release.yml)
[![Github all releases](https://img.shields.io/github/downloads/truehdd/truehdd/total.svg)](https://GitHub.com/truehdd/truehdd/releases/)

Dolby TrueHD 音频流解码工具

**语言:** [English](README.md) | 简体中文 | [日本語](README.ja.md)

> ⚠️ **实验性软件** 
> 
> 本工具仅供研究开发使用，不适用于生产环境或终端用户播放系统。
> 
> 💡 **功能建议**  
> 
> 如有功能改进建议，欢迎通过 issue 或 discussion 反馈。

## 项目简介

`truehdd` 基于 [truehd](truehd/) 库构建，为 Dolby TrueHD 音频流提供命令行解码方案。

## 安装配置

### 源码编译

运行环境要求：Rust 1.87.0 或更新版本

```bash
git clone https://github.com/truehdd/truehdd
cd truehdd
cargo build --release
```

编译后的可执行文件位于：`target/release/truehdd`

## 使用说明

```
truehdd [全局选项] <子命令>

子命令:
  decode    解码 TrueHD 流为 PCM 音频
  info      分析并显示流信息
  help      显示帮助信息

全局选项:
      --loglevel <级别>             日志详细程度 [默认: info]
                                    [可选值: off, error, warn, info, debug, trace]
      --strict                      严格模式（遇到警告即停止）
      --log-format <格式>           日志输出格式 [默认: plain]
                                    [可选值: plain, json]
      --progress                    操作期间显示进度条
  -h, --help                        显示帮助
  -V, --version                     显示版本
```

## 子命令

### `info` - 流分析

分析 TrueHD 流的结构特征，输出详细的技术参数信息而不执行解码操作。

**用法：** `truehdd info [选项] <输入文件>`

```
参数:
  <输入文件>  TrueHD 比特流文件

选项:
...
```

**使用示例：**
```bash
# 分析 TrueHD 文件结构
truehdd info movie.thd
```

### `decode` - 音频解码

解码 TrueHD 流为 PCM 音频。

**用法：** `truehdd decode [选项] <输入文件>`

```
参数:
  <输入文件>  TrueHD 比特流文件（使用 "-" 读取标准输入）

选项:
      --output-path <PATH>       音频和元数据文件的输出路径
      --format <FORMAT>          音频输出格式（表现索引3忽略此选项，始终使用CAF格式）
                                 [默认: caf] [可选值: caf, pcm, w64]
      --presentation <INDEX>     表现索引 (0-3) [默认: 3]
      --no-estimate-progress     禁用进度估计
      --bed-conform              启用Atmos内容的声床适配
      --warp-mode <WARP_MODE>    指定元数据中不存在时的环绕声像延展 (warp) 模式
                                 [可选值: normal, warping, prologiciix, loro]
...
```

**输出文件结构：**

默认情况下，选择最大可用的表现索引进行解码。
指定 `--output-path` 参数后，根据音频流类型生成对应文件：

- **通道表现：** 以下文件之一，表现索引为 0、1 或 2
  - `output.caf` - Core Audio 格式封装的 PCM 数据
  - `output.pcm` - 原始 PCM 数据（需指定 `--format pcm`）
  - `output.wav` - Wave64 格式（需指定 `--format w64`）


- **对象表现：** Dolby Atmos 母版文件，表现索引为 3 （如果存在）
  1. `output.atmos` - 表现的基本信息
  2. `output.atmos.audio` - 所有声床和对象的 PCM 数据，采用 Core Audio 格式
  3. `output.atmos.metadata` - 静态和动态信号的 3D 位置坐标

  **注意：** 表现索引3无视 `--format` 选项，始终使用CAF格式。使用 `--bed-conform` 将声床通道转换为7.1.2布局。

**声像延展模式选项：**

`--warp-mode` 选项控制当 Dolby Atmos 内容的元数据中不包含声像延展模式信息时的降混处理方式：

- `normal` - 直接渲染
- `warping` - 带房间平衡的直接渲染
- `prologiciix` - Dolby Pro Logic IIx
- `loro` - 标准（Lo/Ro）

此选项仅在原始 OAMD 元数据缺少声像延展模式信息时生效。如果元数据中已包含声像延展模式，此选项将被忽略。

**使用示例：**
```bash
# 解码 TrueHD 流并显示进度
truehdd decode --progress audio.thd --output-path decoded_audio

# 为缺少声像延展模式元数据的内容指定特定模式进行解码
truehdd decode --warp-mode prologiciix audio.thd --output-path decoded_audio

# 从 ffmpeg 管道解码
ffmpeg -i movie.mkv -c copy -f truehd - | truehdd decode - --output-path audio
```

## 开源协议

本项目采用 Apache License 2.0 开源协议，详见 [LICENSE](LICENSE) 文件。