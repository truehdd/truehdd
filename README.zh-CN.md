# truehdd

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
      --format <FORMAT>          音频输出格式 [默认: caf] [可选值: caf, pcm]
      --presentation <INDEX>     表现索引 (0-3) [默认: 3]
      --no-estimate-progress     禁用进度估计
...
```

**输出文件结构：**

指定 `--output-path` 参数后，根据音频流类型生成对应文件：

*标准 TrueHD 流：*
- `output.caf` - Core Audio Format
- `output.pcm` - 24 位原始 PCM 数据（需指定 `--format pcm`）

*Dolby Atmos 流：*
- `output.atmos` - DAMF 头文件
- `output.atmos.audio` - CAF 格式音频数据
- `output.atmos.metadata` - DAMF 元数据文件

**使用示例：**
```bash
# 解码 TrueHD 流并显示进度，不输出文件
truehdd decode --progress audio.thd --output-path decoded_audio

# 从 ffmpeg 管道解码
ffmpeg -i movie.mkv -c copy -f truehd - | truehdd decode - --output-path audio
```

## 开源协议

本项目采用 Apache License 2.0 开源协议，详见 [LICENSE](LICENSE) 文件。