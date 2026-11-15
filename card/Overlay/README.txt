# 屏幕遮罩（FC / GB / GBC / SFC）

在 `M → 画面 → 遮罩` 里分别有「FC 遮罩」「GB 遮罩」「GBC 遮罩」「SFC 遮罩」四行，
每行**左右选择**要用的遮罩；可选「无」或对应目录下的任意 PNG，
文件名会显示在菜单行上。

```text
Overlay/FC/    FC 遮罩（例如 FC01.png、FC02.png…）
Overlay/GB/    GB 遮罩（例如 GB01.png、GB02.png…）
Overlay/GBC/   GBC 遮罩（例如 GBC01.png、GBC02.png…）
Overlay/SFC/   SFC 遮罩（例如 SFC01.png、SFC02.png…）
```

## 文件要求

- 位置：卡带根目录下对应的 `Overlay/FC/`、`Overlay/GB/`、`Overlay/GBC/`、`Overlay/SFC/`。
- 扩展名：`.png`。
- 尺寸：**720×480**，与屏幕一致；其它尺寸会被等比裁切。
- 格式：PNG，带透明通道；把要让游戏画面透出的开孔做成**透明**
  （alpha < 0.5），其余部分画边框、机壳、玻璃等。
- 排序按文件名（不区分大小写），菜单里依次是：无 → 第一个文件 →
  第二个文件 → …，也可以左右反向循环。

## 说明

- 一个文件都没有时，对应菜单行只有「无」
  （日志提示一次 `slot: no <机种> mask at …`）。
- 四个机种各自独立选择，保存在 `linkSlot/Config/slot.state`：
  - `fc_mask=<文件名>`
  - `gb_mask=<文件名>`
  - `gbc_mask=<文件名>`
  - `sfc_mask=<文件名>`
  - `none` 表示无；从旧版本升级上来的 `1` 会自动当作第一个遮罩文件。
- 只对对应机种生效；GBA 不支持屏幕遮罩，着色器与调色板不受影响。
