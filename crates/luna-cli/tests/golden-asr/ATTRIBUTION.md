# 转写黄金样本（真实语音）

本目录的音频只用于 `luna bench-asr --reference` 的准确率回归（TDD-13 §2），**不进任何发行物**。

## en/

来源：**LibriSpeech ASR corpus**（`dev-clean-2` 子集，OpenSLR SLR31），V. Panayotov, G. Chen,
D. Povey, S. Khudanpur, "LibriSpeech: an ASR corpus based on public domain audio books",
ICASSP 2015。许可证：**CC BY 4.0**（<https://creativecommons.org/licenses/by/4.0/>）。
朗读文本来自 LibriVox 公有领域有声书。改动：FLAC → 16 kHz 单声道 16-bit WAV，未剪辑。

| 文件 | 说话人 / 章节 | 时长 |
| --- | --- | --- |
| `1272-135031-0015.wav` | 1272 / 135031 | 7.4 s |
| `1462-170142-0002.wav` | 1462 / 170142 | 6.5 s |

参考文本（同名 `.txt`）为语料自带的大写转写，评分前由 `bench-asr` 归一化（小写、去标点）。

## zh/

**暂缺**。候选来源与许可证见 TDD-13 §2（Common Voice CC0 需账号下载；AISHELL-1 Apache-2.0 体量 15 GB，
按需只取数条）。取样时同样在此登记来源、许可证与改动。
