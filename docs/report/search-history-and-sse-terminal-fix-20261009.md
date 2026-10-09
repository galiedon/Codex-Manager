# 搜索历史声明兼容与 Responses SSE 收尾修复

日期：2026-10-09，Asia/Shanghai。源码：`F:\git\Codex-Manager`。

## 证据与版本

参考聊天：`codex://threads/01a11e76-671b-7800-8c91-31fb53a33449`。
参考材料实际位于 `C:\Users\huyihuai\Documents\Codex\2026-10-09\cha\outputs\`：
`codexmanager-web-search-repro.md`、`codexmanager-web-search-repro-evidence.json`。
原提示中的 chenshengyu 路径在本机不存在。

这些对照请求实际使用 `tool_choice="none"`，无压缩标记请求头；同一历史在空 tools 下 3/3 失败、补 web_search 后 3/3 完成。
这证明兼容机制，不证明每条同名错误都由同一原因触发，也不证明原失败会话恢复。

本轮 HEAD 为 `713d5cec69e0acd059971c0e176fa1dbc5193c5d`。
运行服务在 `D:\Software\CodexManager-service-windows-x86_64`，service.exe SHA-256：
`05F1736ADE963ECA4A042AD11FE1D65EFDEC50E6ACECE5E3EDD9E7C29E075B65`，与参考聊天的核查一致。
此前运行版本查询为 0.6.3；本地解引用发布标签 `v0.6.3^{}` 为
`764531421428e35b0fbb3aab8f4f777887e7a44a`。
当前源码与发布标签不同，即使 Cargo 仍写 0.6.3，也不能认为源码与运行二进制相同。
二进制没有提供可核验的源码 commit 标记；标签归属来自既有版本审查，不能仅凭文件哈希反推 commit。

日志页 `http://127.0.0.1:48761/logs/` 本轮只读访问返回 200。
结合该服务的 SQLite 请求日志与 trace，只提取结构及状态，不保存会话正文、认证头或推理密文。
14:29:26 快照截止记录 44775，当天共 2134 条：200=1425、502=130、499=576、507=2、503=1。

| 类别 | 数量 | 证据与边界 |
| --- | ---: | --- |
| 502 protection | 80 | 新近失败如 44753–44758 为 HTTP 200 后 SSE error，tools=0；原 input 正文未保存，不能逐条确认历史 |
| 502 server_error | 23 | 例如 44762：HTTP 200 后 response.failed；不把它归为工具缺声明 |
| 502 解码失败 | 7 | 例如 44767：custom_tool_call_input.delta 后读取失败，未收到终态 |
| 502 其他 | 20 | 含无终态中断、上游 overloaded、cyber_policy；没有绕过上游策略或盲目重放 |
| 499 完成后下游关闭 | 568 | 均观察到 response.completed；原终态交付标记 454=true、114=false，不是客户端收妥的证明 |
| 499 其他 | 8 | 包含排队取消、上游错误同时下游断开、未完成即断开 |

完整结构快照及验证输出在 `artifacts/search-history-fix/`，该目录被 Git 忽略。

## 源码链路与修复位置

1. `http/gateway_endpoint.rs`、`gateway/request/request_entry.rs` 接收请求并进入 local validation；原 client body 的已有捕获保持独立。
2. `gateway/local_validation/request.rs::build_local_validation_result` 读取初始 JSON、判断协议、应用配置覆盖，再适配、标准化并校验。
3. `gateway/request/request_rewrite.rs` 与 `official_responses_http.rs` 在候选转发时填默认值、整理动态工具、保留允许字段；`ensure_tool_choice_auto` 保留明确的 none。
   原有 typed HTTP 标准化会把空 tools/input 数组省略；非原生客户端的兼容规则还会填默认值。此次 HTTP 集成实测确认无搜索时 tools=[] 可被省略，调用权限不变；补齐后的非空 tools 和明确 none 在实际上游请求中保留。
4. `gateway/upstream/attempt_flow/transport.rs` 向实际上游发送请求；普通 ChatGPT Codex 路径映射为 `/backend-api/codex/responses`。
5. `observability/http_bridge/stream_readers/openai_responses.rs` 原样转发 SSE 字节，sidecar 解析 usage 和错误；`openai_responses_event.rs` 把失败识别为错误终态。
6. `upstream/proxy_pipeline/response_finalize.rs` 将 HTTP 200 后的 SSE 失败记为 502、下游交付失败记为 499；保留这套失败分类，不把 completed 观察值当网络 ACK。

### 搜索历史修复

源代码没有能覆盖参考请求的可靠压缩标记。现有 `x-openai-subagent: compact` 会把逻辑路径改为独立 compact，但参考对照请求没有此头。
不根据摘要提示词猜测压缩。用户本轮确认了按零工具调用权限处理的替代范围。

新增 `gateway/request/search_history.rs`，在初始 JSON 已解析、默认值尚未生成时仅修改转发副本：

- 仅逻辑 `/v1/responses`（含 query）且 input 顶层有 completed 的 web_search_call。
- tools 缺少 web_search 或 web_search_* 声明，且 tools 为数组或缺省。
- 当前 choice=none，或显式 tools=[] 且 choice=auto/缺省，并无非空或异常 dynamic_tools。
- 补一个 `{"type":"web_search"}`，把 choice 设为 none。

不删除或重排历史、call ID，不改 reasoning、include、其他工具配置。
已有搜索声明不重复添加或覆盖；普通允许调用工具的请求保持原权限；异常 tools/choice 不修补。
新规则不触发既有模型/推理覆盖策略，也没有新增该类策略。已有配置覆盖仍按仓库原有逻辑生效。
独立 `/responses/compact`、chat/completions、其他路径不应用该补丁；未改现有 compact 路由策略。
原 client metadata 保持原样，转发副本与重试候选保留补齐后的 tools/none。
HTTP 校验的普通与 aggregate 转发分支均使用该副本；WebSocket 请求不在此补丁范围。

### SSE 收尾修复

原普通 Responses reader 等上游 EOF 才结束；终态交付标记只扫描最后 256 字节的关键词，可能漏掉大帧、误认 delta 文本里的 completed，且不能证明完整终态帧已经读完。

新增 `stream_readers/responses_terminal.rs`，跨块解析完整 LF/CRLF SSE 帧并复用既有 JSON 终态判断。
只有有效的完整终态帧才设置观察标记；终态所在 raw chunk 从输出 cursor 全部读完后停止网络输入、排空 sidecar 并返回 EOF。
成功和失败终态都正常结束传输；失败仍通过错误终态与日志分类记为失败。
对没有完成帧的流继续等待或报错；中途客户端关闭继续记为 499。
去掉旧关键词尾部扫描，不改最终状态分类，也不宣称终态观察代表网络送达确认。
这减少终态后的等待和关闭竞态，不能保证历史 568 条 499 都会被消除。

## 修改文件

- `crates/service/src/gateway/local_validation/request.rs`：将修复后的转发副本接入各 HTTP 分支。
- `crates/service/src/gateway/mod.rs`：注册搜索历史模块。
- `crates/service/src/gateway/request/search_history.rs`、`search_history_tests.rs`：规则及 8 个测试。
- `crates/service/src/gateway/observability/http_bridge/stream_readers/openai_responses.rs`、`responses_terminal.rs`：完整帧跟踪及终态后 EOF。
- `crates/service/src/gateway/observability/http_bridge/stream_readers.rs`、`stream_readers/common.rs`：移除旧关键词判断。
- `crates/service/src/gateway/observability/http_bridge/native_async_tests.rs`：模拟上游、完整帧/错误/取消验证。
- `crates/service/tests/gateway/search_history.rs`、`tests/gateway_logs.rs`、`tests/gateway_logs/prompt_cache.rs`：真实本地 HTTP 转发集成测试，复用已有测试种子函数。
- 本报告。没有修改账号、认证、轮换策略、客户端配置或运行目录业务文件。

## 测试

已执行：

| 命令 | 结果 |
| --- | --- |
| `cargo test -p codexmanager-service --lib --locked search_history` | 8 通过 |
| `cargo test -p codexmanager-service --lib --locked gateway::http_bridge` | 147 通过，包括分片/40KB 大终态帧、终态后上游不关闭、HTTP 200 后 error/failed、中途取消及既有 streaming/non-streaming/tools/Chat 适配测试 |
| `cargo test -p codexmanager-service --lib --locked` | 1799 通过、6 失败、8 忽略；全量不是全绿 |
| `cargo test -p codexmanager-web --locked` | 36 通过 |
| `pnpm -C apps run build` | 静态导出成功 |
| `pnpm -C apps run test:runtime` | 250 通过 |
| `cargo test -p codexmanager-service --test gateway_logs --locked search_history_http_forwarding_and_sse_error_logging_regression` | 1 通过；8 次真实本地 HTTP 转发，含连续 4 次新增搜索历史、无历史、已有声明、普通 function/auto，以及 HTTP 200 后 SSE error 最终记为 502 |
| `scripts/release/build-service-binaries.ps1` | Windows x86_64 原生 locked release 构建成功，三个 exe 均生成，Web 静态资源内嵌 |

全量 6 个失败：4 个模型目录/价格断言在修改前已生成的测试二进制中单独同样失败；另外 aggregate-api 模型分配和 codex-profile 两个测试在修改前后单独都通过，尚未证明全量并发环境失败的原因。
不为本次兼容修复修改模型目录、价格数据或客户端配置。详细输出和基线对照均保存在 artifacts 下。

### 可公开的验证日志摘要

以下摘录来自本轮实际执行输出，避免仅写“测试通过”。原始日志位于本地被忽略的 `artifacts/search-history-fix/`，不提交真实请求正文或认证信息。

```text
cargo test -p codexmanager-service --lib --locked search_history
test result: ok. 8 passed; 0 failed; 0 ignored

cargo test -p codexmanager-service --lib --locked gateway::http_bridge
test result: ok. 147 passed; 0 failed; 0 ignored

cargo test -p codexmanager-service --test gateway_logs --locked search_history_http_forwarding_and_sse_error_logging_regression
test result: ok. 1 passed; 0 failed; 0 ignored

cargo test -p codexmanager-web --locked
test result: ok. 36 passed; 0 failed; 0 ignored

pnpm -C apps run test:runtime
tests 250; pass 250; fail 0; skipped 0

cargo test -p codexmanager-service --lib --locked
test result: FAILED. 1799 passed; 6 failed; 8 ignored

scripts/release/build-service-binaries.ps1
Finished release profile [optimized] target(s) in 2m 23s
```

| 全量失败测试 | 全量错误 | 修改前基线单独运行 | 修改后单独运行 |
| --- | --- | --- | --- |
| `gateway::local_models::tests::local_models_lists_each_image_model_once_with_image_only_capabilities` | left=11, right=10 | 同样失败 | 全量同样失败 |
| `quota::model_pricing::tests::catalog_prices_are_exact_and_missing_prices_do_not_fallback` | left=12, right=11 | 同样失败 | 全量同样失败 |
| `quota::read::tests::api_available_model_slugs_does_not_seed_legacy_price_rules` | left=11, right=10 | 同样失败 | 全量同样失败 |
| `quota::read::tests::api_available_model_slugs_preserves_catalog_sort_order` | left=11, right=10 | 同样失败 | 全量同样失败 |
| `aggregate_api::tests::list_aggregate_apis_reads_model_assignments_from_v2_routes_only` | table models already exists | 通过 | 通过 |
| `codex_profile::tests::direct_mode_preserves_previous_catalog_from_config_and_state` | no such table: app_settings | 通过 | 通过 |

基线为修改前已构建的 `codexmanager_service-468a24575964042a.exe`，修改后为 `codexmanager_service-b0066a95ae59f053.exe`；均按表内完整测试名加 `--exact --nocapture` 单独运行。前四行仅有基线单独失败和修改后全量失败证据，未声称做过修改后单独重跑。
这些结果不等于全量通过，也不把两个隔离重测通过的数据库错误直接断言为已修复。

新增规则的连续四次“增加 completed 搜索历史再请求”通过传输标准化与候选重写验证；HTTP 集成测试进一步核对真实送给本地模拟上游的 input/tools/choice，并核对错误记账。
未请求真实上游账号，未部署、未重启，真实客户端恢复尚未验证。

## 打包、部署和回滚

这台机器是 Windows x86_64 服务版，不需要构建 Tauri，也不需要 Docker。
按仓库发布流程执行：

```powershell
pnpm -C apps install --frozen-lockfile
pnpm -C apps run build
./scripts/release/build-service-binaries.ps1
```

Rust 脚本实际执行 locked release build，构建 service、web、start，并开启 `codexmanager-web/embedded-ui`，把 apps/out 嵌入 web.exe。
候选包内三个 exe 与正在运行目录的布局相同。交付包为 `artifacts/search-history-fix/CodexManager-service-windows-x86_64.zip`，不直接覆盖 D: 运行目录。
完整源码补丁为同目录的 `source.patch`，可在指定 HEAD 的干净源码上应用；本轮已做反向 check，确认补丁与当前修改一致。
manifest 标识源码基线、修改文件 SHA-256、运行版旧 service 哈希和新 exe 哈希；不能只用程序显示的 0.6.3 区分候选包。

本包基于当前 HEAD 加本次补丁，不是只给 v0.6.3 发布标签加两行热补丁。
它会包含当前 HEAD 已有的其他变更，包括新 payload 捕获及可能的存储迁移；现有 payload 捕获默认有脱敏和预览，但本轮没有开启或运行候选服务。
部署前需核对已有正文捕获设置及脱敏行为满足不保存真实密文/完整会话的要求，不能在本轮擅自调整用户配置。

部署仅在用户另行确认后进行：

1. 停止 start/service/web 的现有进程，确认释放 48760/48761。
2. 对整个运行目录做离线一致备份（包含 SQLite DB、WAL/SHM、配置、令牌及三个旧 exe）；备份不打印凭据。
3. 仅替换根目录的三个 exe；不要把 zip 内目录再嵌套一层，不删除或覆盖配置、账号库或业务文件。
4. 按原启动方式启动 start，核对 health、Web、manifest 哈希。
5. 真实客户端验证也需确认：保持原历史，多次新增搜索再自动压缩；检查无新工具调用、response.completed 及最终请求日志。
6. 分别观察 protection、server_error、解码失败和 499；server_error/解码失败的根因仍未修复，不能把新包宣称为全部异常的解决方案。

回滚：停止新进程，恢复旧三个 exe。若新版本启动执行了 DB 迁移，需同时恢复同一停机点的完整数据库备份，避免旧二进制读取不兼容的新结构；恢复旧启动方式再核对 health。数据库回滚会丢失部署后产生的数据，需要先保存这一段数据并单独确认。

状态必须分别记录：源码修复及本地验证、候选包生成、已部署、真实客户端验证通过。本轮只完成前两项。
