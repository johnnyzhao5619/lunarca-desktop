# 随包示例语音 — 来源、许可与选样判据

本目录的音频**随应用分发**（`tauri.conf.json` 的 `bundle.resources`），用于首启引导里
「转写一段示例」那条成功路径（PRD-14-051）：它让拒绝了
麦克风、又没有本机语言模型的用户，仍然能在引导内真实地跑通一次本机 AI 任务。

## 选样判据：随包模型必须把它**转对**

🔴 这段音频要充当新用户的「成功时刻」，以及他 Vault 里的第一篇笔记。**转写质量因此是选样
的第一判据，而不是事后才看的指标**——一段读起来像胡话的转写，恰恰会让人得出「这东西不准」
的结论，与这条路径存在的理由相反。

初版曾选 `1272-135031-0015`（同一语料的另一段），随包 tiny-q5_1 在它上面 **WER 23.5%**：
「THE METAL FOREST … DOMED CAVERN … KALIKO」被转成「The Middle Forest … Great Don't
Cavern … replied」。原因是那一段是按**基准样本**挑的——故意含生僻专名，正是小模型最容易
错的东西。用来测缺陷是对的，用来演示就是反效果。现选的这一段在同一模型上 **WER 0.000 /
CER 0.000**（2026-09-11 实测，Apple M5 / Metal）。

🔴 **随包示例必须与 `crates/luna-cli/tests/golden-asr/` 下的某个黄金样本逐字节相同**，由
`scripts/check-bundled-asr-wiring.py` 机器强制。这样换模型档时，既有的真机作业项
`scripts/bench-asr-golden.sh` 会连同这段示例一起报出 CER / WER——不必为准确率另起第二套
门禁（那个 harness 需要 `say` 与 release 构建，刻意不进 CI，见其文件头）。

## lunarca-sample-en.wav

| 项 | 值 |
| --- | --- |
| 来源 | **LibriSpeech ASR corpus**（`dev-clean` 子集，OpenSLR SLR12 / SLR31），原始编号 `1462-170142-0002` |
| 引用 | V. Panayotov, G. Chen, D. Povey, S. Khudanpur, "LibriSpeech: an ASR corpus based on public domain audio books", ICASSP 2015 |
| 许可证 | **CC BY 4.0**（<https://creativecommons.org/licenses/by/4.0/>） |
| 改动 | FLAC → 16 kHz 单声道 16-bit WAV；重命名；**未剪辑、未变速、未降噪** |
| 时长 / 体积 | 6.5 s / 202 KB |
| 朗读内容 | "SHE BLUSHED AND SMILED AND FUMBLED HIS CARD IN HER CONFUSION BEFORE SHE RAN UPSTAIRS"（LibriVox 公有领域有声书） |

署名同时**呈现在界面上**：引导「第一个任务」那张示例卡的脚注写着来源与许可证——
一个只躺在包里、用户永远看不到的通知文件，形式上合规而实质上没有署名。
