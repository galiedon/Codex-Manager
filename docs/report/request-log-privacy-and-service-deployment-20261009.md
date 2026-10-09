# 部署前的请求日志隐私修复

日期：2026-10-09，Asia/Shanghai。基于已推送的 `afa17e434831e60db22e2d7570c093897a0de2f6`。

用户已授权替换及重启服务，随后明确同意先做最小隐私修复：捕获日志仅保留请求结构，不记录会话正文或推理密文，不修改账号和持久配置。

## 原因及实际行为

当前源码基线的正文捕获默认启用。原脱敏规则未覆盖 encrypted_content；16 KB 预览仍可保存完整短请求。脱敏关闭时，溢出队列还会保留原始正文及独立 wire bytes。

新增固定类型的 `structure_only_v1` 摘要：只包含 body_format、input_items、completed_web_search_calls、tools_count、web_search_declarations、dynamic_tools_count、tool_choice、stream、has_encrypted_content。字符串均由固定枚举产生，不复制用户提供的文本。

- SQLite 预处理和 spill 写入均强制使用摘要，独立于原 preview/redaction 设置。preview 仅选择预览行或 manifest 的存储布局。
- spill 不再写 wire bytes；仍保存原始 body/wire SHA-256、字节长度和实际转发 attempt 元数据。已有 URL 去除用户名、密码、query 和 fragment 的行为保持不变。
- 摘要可幂等重放；无效 JSON、根数组、纯文本及二进制均不会原样记录。固定 schema 拒绝用伪造摘要夹带任意字符串。
- Gemini trace 的可选正文预览也使用同一摘要。普通请求 trace 正文预览原先就不输出正文，保持原行为。
- 修改只发生在日志写入边界，保留实际上游的完整历史、调用 ID、工具配置和推理字段。客户端与上游结构可对照 tools=0/choice=auto 到搜索声明=1/choice=none。

未修改账号、认证、轮换策略、客户端配置或持久设置。既有数据库及 spill 里的历史内容不自动清除；本次保护新写入的记录，旧记录清理需要用户另行授权。

## 验证记录

原先断言保存明文、会话正文或逐项历史的服务测试，更新为结构摘要断言；保留字节哈希、转发内容、清理时序、队列顺序、故障重试等独立验证。

实际执行：

| 验证 | 结果 |
| --- | --- |
| `cargo test -p codexmanager-service --lib --locked request_log_payload -- --test-threads=1` | 41 通过，含结构隐私 4 项，以及脱敏关闭时实际 spill 落盘/重放验证 |
| `cargo test -p codexmanager-service --test gateway_logs --locked -- --test-threads=1` | 53 通过，152.29 秒；含连续搜索历史修复及 HTTP client/upstream 两阶段结构日志对照 |
| 同一新构建 lib 测试二进制，filter `gateway::http_bridge` | 147 通过 |
| 同一新构建 lib 测试二进制，filter `search_history` | 8 通过 |
| 同一新构建 lib 测试二进制，filter `gateway::trace_log` | 10 通过 |
| `http::proxy_runtime::tests::official_responses_websocket_proxies_frames_and_headers`，`--exact` | 1 通过 |
| `http::proxy_runtime::tests::official_responses_websocket_switches_account_after_initial_send_reset`，`--exact` | 1 通过 |

初次隐私测试的 11 个失败来自旧断言仍要求保存正文；同步为新的隐私契约后 41 项全部通过。上一次 service 全量的 6 项失败及基线对照见 `search-history-and-sse-terminal-fix-20261009.md`；本轮未重跑 service 全量，不能将针对性测试等同于全量全绿。

## 构建与部署边界

运行目录：`D:\Software\CodexManager-service-windows-x86_64`。只替换根目录的 service/web/start 三个 exe，按原启动器启动，检查 48760/48761 的 health、Web 和文件哈希。

已在服务离线时完整备份到 `D:\Software\CodexManager-backups\20261009-152729`，20 个文件均逐一核对哈希。替换前再次确认离线状态及备份与业务文件的一致性；若发生新写入，重新备份。

最终构建 commit、三个 exe 哈希、进程、health 结果、是否实际部署，记录在本地 `artifacts/search-history-fix/deployment-receipt.json`。原始验证输出只包含合成测试数据，不提交二进制、备份、运行密钥或真实会话。

回滚需先停止新 start/service/web 并恢复旧三个 exe。若新启动执行了数据库迁移，应使用同一离线备份恢复数据库；恢复前先保存部署后数据，涉及丢弃新增数据时另行确认。不能在新进程运行时混拷数据库与 WAL/SHM。

本次重启授权不包含主动真实账号调用。health/Web 检查只验证本地运行状态；原客户端连续搜索及自动压缩恢复仍待单独验证。
