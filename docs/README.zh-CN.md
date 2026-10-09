# prof-mcp

[English](../README.md)

工作区本地的 stdio MCP 服务器，让 agent 以有界、结构化、确定性的方式查询折叠栈（folded stack）profile。在 Linux 上，`capture` 可一步完成 `perf` 采集并注册。

英文 README 是权威版本，本文是其摘要翻译。

## 快速开始

将 `prof-mcp` 放入 `PATH`，然后注册到 Codex：

```bash
prof-mcp setup            # 加 --dry-run 可预览
```

`setup` 只修改 Codex 的 MCP 条目（等价于 `command = "prof-mcp"`、`args = ["serve"]`），幂等；如果已有同名条目且带自定义参数、环境变量、工作目录或超时，会拒绝覆盖。

在工作区中注册 profile：

```bash
prof-mcp register ./perf.folded                    # 别名默认取自文件名
prof-mcp register ./baseline.folded --name baseline
producer | prof-mcp register - --name streamed     # 从 stdin
prof-mcp capture --name candidate -- ./my-program args...   # 仅 Linux，需要 perf
```

`prof-mcp PROFILE` 是 `register` 的简写；无参数运行只打印帮助。服务器每次查询都会向上查找最近的 `.prof-mcp/manifest.json`，注册后无需重启。尚无 profile 时，查询返回 `workspace_not_registered`。

## 管理 registry

```bash
prof-mcp list
prof-mcp use baseline
prof-mcp remove candidate                          # 只删别名；blob 留给 gc
prof-mcp remove baseline --new-active candidate    # 删除当前别名时必须指定
prof-mcp gc --dry-run
prof-mcp gc                                        # 只删除未被引用的 blob
```

Profile 以原始字节存放在 `.prof-mcp/profiles/<blake3>.folded`，相同内容自动去重。最后一个别名不能删除。变更操作持有持久化的咨询锁，manifest 原子写入。生成的数据被 git 忽略，registry 内的小 `.gitignore` 保持可见。

## 工具

共八个，顺序固定：

| 工具 | 作用 |
| --- | --- |
| `profile_summary` | 别名、指纹、总权重、profile 形状 |
| `profile_find_symbols` | 子串或正则搜索，得到精确 frame 与统计 |
| `profile_top` | 按 `self` 或 `inclusive` 排名；`frame` 限定为包含该 frame 的栈 |
| `profile_tree` | 有界上下文树 |
| `profile_callers` / `profile_callees` | 某个精确 frame 的调用者 / 被调用者链 |
| `profile_paths` | 经过某 frame 的最重完整栈 |
| `profile_diff` | 比较两个别名（必须都显式给出） |

单 profile 工具默认使用 active 别名。建议 agent 按以下顺序使用：

1. `profile_summary`：确认别名、指纹、总权重。
2. `profile_find_symbols`：获得精确名称或 frame ID（ID 只属于对应 profile）。
3. `profile_callers` / `profile_callees` / `profile_paths`：查看调用上下文。
4. `profile_top` 配合 `frame` 与 `metric:"self"`：查看该范围内的 self 排名。
5. `profile_diff`：比较两个明确指定的别名。

inclusive 权重高不等于优化收益；diff 也不证明因果或统计显著性。结果只描述所观测的 profile。

被截断的 tree / callers / callees 结果会返回 continuation。把返回的 `data.continuations[i].continuation` 原样复制到下一次请求的 `continuation` 字段，并省略 `profile` 和 `frame`。若别名已指向不同字节，需要不带 continuation 重新开始。不要自行拼装 cursor。

`profile_paths` 的 `frame_window` 只裁剪显示，不改变路径选择和权重（示例见英文 README）。输入 schema 拒绝未知字段。响应的 schema 版本为 `"2"`，`truncated=true` 时一定带结构化的 `truncation_reasons`。

客户端必须把 `structuredContent` 暴露给 agent：完整证据和错误恢复提示都在其中；文本 `content` 只是最多 2048 字节的摘要。实际宿主（包括 Codex）如何映射两者尚未验证。

## 限制

- 输入 profile：默认 512 MiB（`--max-file-size-mib`），单行 8 MiB，每个栈最多 4096 帧，总权重不超过 `2^53 - 1`。
- 工具结果：压缩 JSON 最多 64 KiB，超出返回 `query_too_large`；请降低行数/节点/帧数限制或收窄查询。
- Manifest：4 MiB（`registry_too_large`）；别名缺失错误最多列出 100 个别名。
- 各工具的行数、节点数、深度上限见生成的输入 schema。

## 折叠格式与 capture

每行形如 `frame;frame weight`，权重为正整数；frame 是不透明且区分大小写的标识，权重无单位。

`capture` 直接把命令交给 `perf record -g`（不经 shell），并在 Rust 中折叠 `perf script` 输出：保留完整函数签名与 `[module]` 后缀，只去掉末尾的 `+0xHEX` 偏移；未知符号记为 `[unknown@0xADDR]`；字面量 `% ; [ ]` 被百分号转义（`%25 %3B %5B %5D`），查询时请使用返回的编码名称。混合事件类型、格式错误、超限或 perf 失败都会使 capture 失败，且不影响已有别名。

## 平台支持

Registry 操作目前需要 Unix；`capture` 另外需要 Linux。CI 仅验证 Linux，其他平台未验证。

## 开发

开发使用 `rust-toolchain.toml` 指定的 stable 工具链，最低支持 Rust 1.88，CI 同时测试两者。

```bash
cargo build --locked --release
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets --all-features
cargo test --locked --release --test stdio     # 真实二进制的 MCP 冒烟测试
```

- 模糊测试：[fuzz/README.md](../fuzz/README.md)
- perf 对比与 agent 评测：[eval/README.md](../eval/README.md)
- MCP Inspector：`inspector.config.json` 用于在仓库根目录调试；检查其他工作区时，把 `command` 改为绝对路径。

本项目处于 1.0 之前，公开的 Rust 模块属于实现细节。

## 许可证

[MIT](../LICENSE)
