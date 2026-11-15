# Labels（卡带标签）

把 `.png` 放进 `GBA/`、`GB/` 或 `GBC/`，文件名与 ROM 相同。标签在所有设备上通用。

  Games/GBA/Metroid Fusion.gba  ->  Labels/GBA/Metroid Fusion.png

  卡带              比例      建议尺寸
  GBA               1.94:1    1280 x 661
  Game Boy          1.13:1    1024 x 901
  Game Boy Color    1.12:1    1024 x 901
  Famicom / NES     1.53:1    1024 x 670

其他尺寸会按比例缩放、从中心裁切。

# Shells（外壳）

`cart_shell.txt` 按每行一个游戏设置外壳：

  Metroid Fusion = auto 6a3fa0 solid

  Outline    auto、notched 或 rounded
  Colour     十六进制 RGB
  Finish     solid、clear 或 glitter
