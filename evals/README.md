# evals —— 能力提示词的离线评测集（ADR-050）

提示词从代码里的字面量升级成**可调优的产品配置**之后，多出来一个问题：
**怎么知道新一版比旧一版好？** 没有答案的话，「不发版即生效」只是把改坏的速度也提高了。

本目录是那个问题的最小可执行答案：**固定用例集 + 可重复执行的打分脚本**。

## 跑

```bash
python3 evals/score.py
```

零依赖、零网络、确定性——同一份模板跑十次得到同一个分数。它读的是
`crates/luna-core/prompts/bundled.json`（随包默认那一版）；要评一份**候选发布**，
把管理台里那份 JSON 存下来传进去：

```bash
python3 evals/score.py --prompts /path/to/candidate.json
```

## 它评什么，不评什么

**评的是提示词的结构性质**——那些「错了必然坏、且不需要模型就能判定」的东西：

| 检查 | 为什么它值得一条断言 |
| --- | --- |
| 占位符齐备 | 模板要 `{target_lang}` 而组装点不给，模型收到的就是字面写着 `{target_lang}` 的指令 |
| 无悬空占位符 | 组装点给了 `{structure}` 而模板不用它，「保留换行」这条纪律就静默消失了 |
| 必备指令在场 | 翻译模板丢掉「不要回答问题」，小模型会把 `how are you?` 当成对话来答 |
| 渲染后不残留 `{…}` | 上面两条的兜底：真渲染一遍，看结果里还有没有花括号 |
| 语言一致性 | 目标语言必须以**全称**出现（小模型对 `into English` 远比 `into en` 敏感） |

**不评的是输出质量**——那需要跑模型，而模型不确定。质量由人在晋升前看：流程见下。

知识库主题初稿另有一个可选的本机合成探针：

```bash
python3 evals/topic_generation_probe.py --model qwen3.5-4b --repeats 3  # 先自起一个 llama-server
```

主题候选的检索层实验（PRD-16-063）：60 组来源去掉标签后，「正文写到主题名」与 bge-m3 语义相似度各能把多少同主题来源找回来、会混进多少别的主题的来源。只是诊断，写不写、写得对不对仍归所有者验收。

```bash
python3 evals/topic_candidate_probe.py             # 词法规则，离线
python3 evals/topic_candidate_probe.py --semantic  # 加 bge-m3 对照（需本机 llama-server --embeddings）
```


它只向本机 `llama-server`（`LUNA_LLAMA_BASE_URL`，缺省 `http://127.0.0.1:8080/v1`）发送 `fixtures/topic_generation_model.json` 中的固定虚构材料，
检查同源双项目、无关主题 no-op 与输出格式。它绕过应用的 Provider、JobQueue、
ResultSchema、Vault 写入与真实 UI；小样本通过也不代表 PRD-16 的语义质量或端到端验收。
失败时原始模型输出只显示在命令行，夹具不得换成用户正文。

需要检验真实 `AIProvider` 的事件终态时，用同一夹具跑默认忽略的实时测试：

```bash
LUNA_LLAMA_BIN_DIR=<引擎目录> LUNA_LLAMA_CHAT_GGUF=<4B 的 .gguf> LUNA_LLAMA_CTX=16384 cargo test -p luna-provider --test llm_live topic_generation_provider_smoke -- --ignored --nocapture
```

这条测试在隔离 Vault 提取证据，核对给 Provider 的片段，再检查未截断的 `Finished`
结果能否通过封闭解析并形成只读预览；它仍绕过应用 `JobQueue` 和正式初稿命令，
不覆盖增量、持久计划或所有者的语义验收。

既有主题的增量（`ingest.topic_update`）另有逐次打分的实时探针，夹具为
`fixtures/topic_update_model.json`（追加 + 分歧、只含重复、无关范围三类）：

```bash
LUNA_LLAMA_BIN_DIR=<引擎目录> LUNA_LLAMA_CHAT_GGUF=<4B 的 .gguf> LUNA_LLAMA_CTX=16384 LUNA_PROBE_REPEATS=3 cargo test -p luna-provider --test llm_live topic_update_provider_probe -- --ignored --nocapture
```

它不因语义失误失败，而是逐行打印 `PROBE … pass=` 与汇总 `PROBE_SUMMARY`：协议错误、
范围外引用、缺失的分歧指向分别标出，协议失败时附原始输出前缀（夹具全为虚构内容）。
Rust 只能拦下逐字重复；换一种说法的重复与范围判断仍靠模型，须由所有者在固定评测集上判定。

## 语义质量的人工验收（PRD-16 §4）

要求本身在 PRD-16 §4（所有者本人判断、按模型/提示词版本分别报告）；这里只写**怎么做**：

1. **材料**：只用虚构或经本人同意的样本，逐条记下所用 `model_revision` 与 `prompt_revision`（封存计划里都有）。不收集真实用户正文（ADR-018）。固定评测集是 `fixtures/topic_review_set.json`：60 组虚构样本，笔记 / 速记 / 录音 / 导入转写各 15 组，覆盖范围内、多主题同源、无关、重复、同名不同义、时间变化（替换）、分歧、确定程度与否定。跑批导出一份待标 CSV（每条新声明一行，带所引片段与三列机器初筛——引用是否落在本主题、是否出现范围外字样、确定程度/否定词是否保留；机器初筛只帮评审定位，不替代判断）：

   ```sh
   LUNA_LLAMA_BIN_DIR=<引擎目录> LUNA_LLAMA_CHAT_GGUF=<4B 的 .gguf> LUNA_LLAMA_CTX=16384 cargo test -p luna-provider --test llm_live \
     topic_review_set_export -- --ignored --nocapture
   ```

   输出缺省写到 `review/out/<模型>.csv`（已忽略，不入库）；设 `LUNA_COMPUTE_BASE_URL` / `LUNA_COMPUTE_API_KEY` / `LUNA_COMPUTE_MODEL` 则改连该端点。
2. **每条声明四个字段**（空白表见 `review/annotation-template.csv`；跑批导出的末六列与它同名同序）：
   - `relevant`：是否属于该主题范围（同源双项目时不得串到另一个项目）；
   - `supported`：所引片段是否直接支持这句话；
   - `certainty_kept`：确定程度是否与来源一致——来源只说「希望/建议/请求」却写成「计划/决定/已定」记为否；
   - `conflict_kept`：遇到分歧是否并列保留、没有替用户选边（问答同样适用）。
   取值只有 `yes` / `no` / `na`，另填一句理由。
3. **标注**：所有者在导出的 CSV 上逐条填写，不看任何模型预标（预标会让人向它靠拢）。
4. **报告**：`python3 evals/review/report.py 标注后.csv` 按 `model_revision × prompt_revision` 分组给出四个字段的通过率与样本数，不把不同版本混在一起算。所有者看过点头即放行，不设评审人数与统计门槛。

真机已见的三类失误可作首批反例（改写为虚构内容再入集）：把「客户希望」写成「项目计划」（`certainty_kept`=no，两例）、问答按「当前主流」替用户选边并抄出系统加注（`conflict_kept`=no，一例）。

## 🔴 刻意不做：生产流量的正文级 A/B

给两组真实用户发不同模板、再比较他们的输出好坏——那需要**留存用户正文**，
与 ADR-018 的零留存承诺正面冲突。理由与代价写在 ADR-050 的「刻意不做」。
可做的替代：按 `prompt_version` 分组看**时延与 token 分布**（管理台用量视图已支持），
那是纯计量数字，一个字节的正文都不需要。

## 晋升流程（写进流程，不靠自觉）

一份提示词发布晋升到 `Active` 之前必须：

1. `python3 evals/score.py --prompts <候选>` 全绿——**服务端在 `Staged → Active` 时按同一套用例
   再跑一遍并硬闸**（商业仓 `cp_domain::prompt_evals`，管理台「评测」按钮随时可看），本地这一步
   是为了早点看到红，不是唯一防线；
2. 人工在管理台的**版本 diff** 里逐条看过改了什么；
3. 用几条真实素材手动跑一遍四能力（真机，不在自动化范围内）。

改坏了的止血动作是管理台的**一键回滚**（它会把旧内容重新签一个更大的版本，
而不是把指针挪回去——挪指针会被客户端防回滚拒绝，等于什么都没做）。

## 用例集

`cases/*.json`，每个能力一份。当前是**精简起步**：结构先行，条数随真实反馈增长。
加用例只需往数组里追加，不改脚本。
