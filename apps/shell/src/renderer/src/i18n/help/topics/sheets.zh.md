# Sheets：电子表格

Sheets 是类 Excel 的表格编辑器，计算由一个独立的 Rust 引擎进程承担（崩溃不影响主程序）。打开与保存都是真 .xlsx；.csv 和 .tsv 会作为表格打开。

## 界面

- **功能区**：开始页签里是字体、填充、边框、数字格式、对齐、行列与工作表操作等分组。
- **公式栏**：显示与编辑当前单元格的公式；支持常用函数。
- **工作表标签**（底部）：新建 / 重命名 / 删除 / 移动工作表。
- **单元格编辑**：双击或直接输入；Enter 确认下移、Tab 右移、Esc 取消（Excel 习惯）。
- **快捷键**：与 Excel 家族对齐（ctrl+C/V/X、ctrl+Z/Y、ctrl+F 查找等），详见应用内提示。

## 数字与格式

- 数字格式：常规、数值、货币、百分比、日期时间、分数、科学计数等。
- 对齐、换行、合并单元格、边框与填充色。
- 行高列宽拖拽调整，双击边界自适应。

## 数据

- 排序与筛选。
- 冻结窗格。
- .csv / .tsv：直接打开为表格（制表符分隔的 tsv 按表解析），保存回原格式。

## 脚本编辑器（进阶）

Sheets 内置一个**脚本编辑器**：用类 Google Apps Script 的 API 批量处理数据。

- 打开：菜单 工具 ▸ 脚本编辑器（或开发者菜单，随版本）。
- 界面：左侧脚本库（可新建/删除），右侧代码编辑器 + 输出窗格；**运行 / 停止** 按钮。
- API 形态（异步）：

```js
const sheet = await SpreadsheetApp.getActiveSpreadsheet()
const active = await sheet.getActiveSheet()
const range = await active.getRange('A1:C10')
const values = await range.getValues() // 二维数组
await range.setValues(values.map((row) => row.map((v) => v * 2)))
Logger.log('done')
```

- `SpreadsheetApp`（入口）、`Sheet`（getName/getRange/getLastRow…）、`Range`（getValue(s)/setValue(s)/clear…）、`Logger.log`、`Utilities.sleep`。
- 脚本运行在**沙箱 Worker**里：没有网络、没有文件系统、没有 DOM，只能通过上述 API 触达表格——恶意或写错的脚本动不了你的其他数据。
- 输出窗格有行数上限，防止巨量日志拖垮界面。
- **AI 也能调用脚本**：AI 助手的 `run_script` 工具执行同样的沙箱 API，适合批量/规则化改写。

## AI 能力

- 右侧 AI 面板：选中区域后用自然语言下指令（改格式、生成数据、写公式等）。
- AI 的修改可以通过面板回滚。

## 保存与导出

- 保存为 .xlsx（公式与格式保留）；另存为、导出 PDF 按打印分页。
- 自动保存遵循全局规则（首次手动保存后开启）。

## 稳定性

- 计算引擎（Rust 边车）与界面进程隔离：极端数据导致引擎退出时，界面会提示并尝试恢复会话，不会连坐整个应用。
