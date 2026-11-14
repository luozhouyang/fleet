# Agent Fleet — Daylight Bench 设计规范

**状态：** 设计稿（v0.3，session-first 工作台）
**日期：** 2026-11-14
**实现目标：** shadcn（Base UI）+ Tailwind CSS

---

## 1. 方向

主对象是 **Session**。界面不是后台管理表格，而是一张白天的工作台：

- 左侧是常驻的 session 列表，按 machine / workspace 分组。
- 中间是当前 session 的对话，采用 logbook 式时间线，而不是聊天气泡流。
- 右侧是可展开 / 折叠的上下文面板：File、Diff、System 三个视图。

视觉语言：暖纸底、石墨墨色、单一余烬橙强调色。深度来自**表面层叠 + 柔和环境阴影**，不靠描边堆叠，也不使用装饰性色条。

### 硬规则

1. **没有左侧 indicator。** 会话行、权限卡、抽屉、状态列都不使用彩色竖条。
2. **状态只由 6px 圆点 + 文字承担。** 不做彩色胶囊、不做色块背景。
3. **强调色只表示动作与选中。** 运行 / 等待 / 失败使用独立状态色。
4. **1px 描边只出现在卡片外轮廓。** 内部用留白、字重和极淡分隔线。
5. **颜色只承担语义，装饰交给排版。**

---

## 2. 色彩 token（最终采用 B · Bone & Ember）

### 2.1 语义色

| Token | Hex | 用途 |
|---|---|---|
| `paper` | `#F4F1E8` | 应用画布 |
| `paper-side` | `#F0ECDF` | 左侧栏 |
| `surface` | `#FFFDF8` | 卡片、输入框、右面板内容 |
| `surface-inset` | `#ECE8DC` | 代码块、凹陷区域 |
| `surface-muted` | `#F1EDE2` | 次级表面 |
| `ink` | `#1B1A14` | 标题与高强调文字 |
| `ink-body` | `#444135` | 正文 |
| `ink-muted` | `#6B6657` | 次级信息 |
| `ink-faint` | `#97927F` | 占位、弱元数据 |
| `line` | `#E4DFD1` | 面板 / 卡片描边 |
| `line-strong` | `#D5CEBC` | 分组边界 |
| `primary` | `#B64A25` | 主按钮、选中、链接、焦点 |
| `primary-hover` | `#9E3F1E` | hover / pressed |
| `primary-soft` | `#F7E8E0` | 用户消息底、浅强调底 |
| `selected` | `#F7EDE4` | 列表选中行 |
| `success` | `#2E6B4C` | Running |
| `success-soft` | `#E4EFE7` | diff add / 成功底 |
| `warning` | `#8E620A` | Waiting / 审批 |
| `warning-soft` | `#F6ECD5` | 审批卡底 |
| `danger` | `#A32C22` | Failed / 删除 |
| `danger-soft` | `#F8E6E2` | diff remove |

### 2.2 shadcn 映射

```css
@theme {
  --color-background: #f4f1e8;
  --color-foreground: #1b1a14;

  --color-card: #fffdf8;
  --color-card-foreground: #1b1a14;
  --color-popover: #fffdf8;
  --color-popover-foreground: #1b1a14;

  --color-primary: #b64a25;
  --color-primary-foreground: #ffffff;
  --color-secondary: #f1ede2;
  --color-secondary-foreground: #1b1a14;

  --color-muted: #ece8dc;
  --color-muted-foreground: #6b6657;
  --color-accent: #f7e8e0;
  --color-accent-foreground: #1b1a14;

  --color-destructive: #a32c22;
  --color-destructive-foreground: #ffffff;

  --color-border: #e4dfd1;
  --color-input: #e4dfd1;
  --color-ring: #b64a25;

  --color-sidebar: #f0ecdf;
  --color-sidebar-foreground: #444135;
  --color-sidebar-accent: #f7ede4;
  --color-sidebar-border: #e4dfd1;

  --color-chart-1: #b64a25;
  --color-chart-2: #2e6b4c;
  --color-chart-3: #8e620a;
  --color-chart-4: #6b4e8f;
  --color-chart-5: #97927f;

  --radius: 0.625rem; /* 10px */
}
```

- 焦点环：`ring-2 ring-ring/45 ring-offset-2 ring-offset-background`。
- 状态色不进入 shadcn 的 `primary`，用独立 utility（`text-success`、`bg-success-soft` 等）。
- 所有数字使用 `font-variant-numeric: tabular-nums`。

### 2.3 对比度

| 组合 | 对比度 |
|---|---|
| `ink` on `paper` | 15.4:1 |
| `ink-body` on `paper` | 8.7:1 |
| `ink-muted` on `paper` | 4.8:1 |
| `primary` on `surface` | 5.0:1 |
| white on `primary` | 5.0:1 |
| Running / Waiting / Failed 文本与各自淡底 | 5.0–5.8:1 |

---

## 3. 字体

| 角色 | 字体 | 字重 | 用途 |
|---|---|---|---|
| Display | **Archivo** | 700 | 页面标题、session 标题、数字 |
| UI / Body | **Ubuntu** | 400 / 500 / 700 | 正文、按钮、列表 |
| Data | **Noto Sans Mono** | 400 / 500 | 路径、hex、耗时、diff、元数据 |

> 离线环境只有 Archivo Bold 可用；实现阶段可引入完整 Archivo 字重。
> 代码编辑器字体建议 `Berkeley Mono` 或 `JetBrains Mono`，设计稿暂用 Noto Sans Mono。

| 样式 | 规格 |
|---|---|
| page-title | 30/36, Archivo 700, tracking −1.5% |
| panel-title | 18/24, Archivo 700, tracking −1% |
| body | 14/21, Ubuntu 400 |
| ui | 12.5/18, Ubuntu 400/500 |
| micro | 9/12, Noto Sans Mono, tracking +6% |
| code | 11/18 (编辑器), 8.5/15 (右栏 diff), Noto Sans Mono |

---

## 4. 布局

### 4.1 三栏工作台（≥ 1280px）

```
┌──────────────────────────────────────────────────────────────┐
│ TopBar 56                                                    │
├──────────────┬────────────────────────────────┬──────────────┤
│ Sidebar 320  │ Conversation flex              │ Panel 400    │
│ session list │ header / timeline / composer   │ File Diff Sys│
└──────────────┴────────────────────────────────┴──────────────┘
```

- **Sidebar 320**：`Sessions` 标题、搜索、过滤、按 machine/workspace 分组、底部连接状态。
- **Center**：会话标题 19px，时间线，底部 composer。对话内容最大宽度 680，在折叠右栏时保持居中。
- **Right Panel 400**：顶部 tabs（File / Diff / System）+ 折叠按钮；内容独立滚动。
- 分隔只使用 1px `border` 竖线，不使用卡片包裹整栏。

### 4.2 折叠态

右栏折叠为 **44px 图标轨**：展开箭头 + File / Diff / System 三个图标。中间对话区扩展到 1074px，内容仍保持 680 宽居中。

### 4.3 响应式

| 宽度 | 行为 |
|---|---|
| ≥ 1280 | 320 + center + 400 三栏 |
| 960–1279 | 侧栏 280，右栏 360 |
| 720–959 | 右栏变为覆盖式 drawer（`sheet`），默认收起 |
| < 720 | 侧栏变 `sheet`，进入单栏会话视图 |

---

## 5. 组件规范

### Session row（左栏）

- 高度 50，左右 padding 10，圆角 8。
- 第一行：session 名 12.5/700。
- 第二行：6px 状态点 + 状态文字 + `·` + `12s ago`。
- 选中：`bg-selected`，名称变 `primary`。无左侧色条。
- hover：`bg-surface-muted`，150ms。

### Status

- 仅 `圆点 + 文本`，文本使用状态色，字重 700，字号 9–10。
- Running 状态点可做 2.4s 呼吸（`opacity 1 → 0.55`），其余静态。

### Conversation turn（logbook）

- 左侧 gutter 42px：时间（mono 9）+ 角色（mono 8，accent 色）。
- 用户消息：`bg-primary-soft`，圆角 9，padding 12，最大宽度 600。
- Agent 消息：无气泡，正文 12.5/19。
- 工具调用：`surface` 卡片 + 1px 边框 + `shadow-sm`；头部左侧工具名/路径、右侧耗时；代码块 `surface-inset`。
- 权限卡：`warning-soft` 底，圆角 10，无左侧色条；命令块用 `surface`。
- 工作状态：圆点 + `Running tests` + 计时 + `interrupt` 链接。

### Composer

- 宽 640，圆角 10，`surface`，`shadow-md`。
- 上：placeholder；下：附件、模型、`/ commands`、发送按钮（primary 28×28，圆角 7）。
- 聚焦：`ring-2 ring-ring/45`。

### Right panel

- Tabs：File / Diff / System，11px；激活项加 2px primary 下划线。
- Diff：文件列表 28px 行高，选中行 `selected` 底；diff 行 17px，add/remove 使用淡底，不换行（超出横向滚动）。
- System：provider、model、workspace、branch、permissions、tokens、audit。
- 折叠：200ms `cubic-bezier(.22,1,.36,1)`，宽度 400 → 44。

### Approval

- 遮罩：`ink` 45% 不透明度。
- 弹窗 560，圆角 16，`shadow-lg`。
- 标题区：警示图标 + 权限类型 + 标题；命令块；文件清单；风险说明；`Remember for this session`；底部 Reject / Approve。
- 主按钮使用 `primary`，不使用 `danger`——危险语义由文案和风险说明承担。

---

## 6. 动效

| 场景 | 规格 |
|---|---|
| 通用 | 180ms `cubic-bezier(.22,1,.36,1)` |
| 列表进入 | 8px 上浮 + 淡入，20ms stagger，最多 8 项 |
| 右栏展开/折叠 | 220ms，宽度与内容淡入同步 |
| 卡片 hover | translateY(−1px) + shadow 加深，150ms |
| 焦点 | 2px ring + 2px offset，120ms |
| 状态点 | 2.4s ease-in-out 呼吸（仅 Running） |

`prefers-reduced-motion: reduce` 时关闭位移与呼吸，只保留透明度变化。

---

## 7. 页面清单

1. **W1 Workbench** — 左 session 列表 + 中对话 + 右 Diff 面板（展开）。
2. **W2 Workbench collapsed** — 右栏折叠为 44px 图标轨。
3. **W3 Approval** — 工作台之上的审批弹窗。
4. **Workspaces** — 按 machine 分组的 ledger（不是卡片网格）：workspace / branch / changes / sessions / last activity。
5. **Workspace** — 三栏工作区：文件树 / 编辑器 / Changes + commit。
6. **Machines** — machine ledger + 最近会话 ledger。
7. **Machine** — providers / workspaces / recent sessions + 右侧 health 面板。

**Sidebar 只属于 Session 工作台。** 所有页面共用同一个 56px TopBar；Workspaces / Machines 使用全宽内容区，不再嵌套第二层侧栏。

### 交付物

| 文件 | 内容 |
|---|---|
| `docs/design/w1-workbench.png` | 工作台展开态（Diff） |
| `docs/design/w2-collapsed.png` | 工作台折叠态 |
| `docs/design/w3-approval.png` | 审批弹窗 |
| `docs/design/p-file.png` | 右栏 File tab |
| `docs/design/p-system.png` | 右栏 System tab |
| `docs/design/s4-workspaces.png` | Workspaces ledger |
| `docs/design/s5-workspace.png` | Workspace 三栏 |
| `docs/design/s6-machines.png` | Machines ledger |
| `docs/design/s7-machine.png` | Machine 详情 |
| `docs/design/palette-systems-v0.2.png` | 三套色板对比（B 已选中） |

---

## 8. shadcn / Base UI 对应

| 设计元素 | 实现 |
|---|---|
| 左侧栏 | `Sidebar` + `ScrollArea` |
| 三栏尺寸 | `ResizablePanelGroup`（可选，固定宽度也可） |
| 会话分组 | `Collapsible` |
| 右栏 tabs | `Tabs`（Base UI `Tabs`） |
| 右栏折叠 | `Tooltip` + `Button variant="ghost"` |
| 审批 | `Dialog` + `Alert` + `Checkbox` |
| 命令 | `Command`（⌘K） |
| 状态 | `Badge` 的纯文本变体或自定义 `StatusDot` |
| diff | 自建 `DiffLine`，颜色用 success/danger token |
| 代码 | Monaco / CodeMirror，主题从 token 生成 |

---

## 9. 验收

- 任何页面都不出现装饰性左侧色条。
- 状态只由圆点和状态色文字表达。
- 主色只出现在主动作、选中、链接、焦点。
- 右栏可在 400 / 44 两种宽度间切换，中间内容保持可读宽度。
- 全部文本对比度 ≥ 4.5:1；大字号 ≥ 3:1。
- 键盘可达：`⌘K` 命令面板、`⌘↵` 审批、`Esc` 关闭右栏/弹窗、Tab 焦点环可见。
