In thirty seconds: your laptop may already be able to run a large language model.

## {{h_hook}}

[CAM] You might think running an LLM takes a graphics card that costs thousands. The 16 GB laptop in front of you can run an 8B model today.

> [!retake] Example: this line was read too fast while recording; when follow-along stops, marks are written back here.

## Bandwidth is the bottleneck

[VO] For every word it generates, the model reads all of its weights from memory once. So what decides speed is how much data memory can move per second, not how fast the chip computes.

```broll
Screen recording: text being generated alongside memory-bandwidth usage.
```

## Unified memory skips the copying

[VO] In a traditional computer, GPU memory and system memory are separate, and the model has to be copied into GPU memory first. Unified memory merges them, so the model runs right where it sits.

## {{h_outro}}

[CAM] So next time you buy a computer, check how much memory it has and how fast it is. See you next time.
