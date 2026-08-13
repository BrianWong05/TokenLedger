// Limits-tab strings. en is the key universe, zh-Hant is natural Traditional
// Chinese for a Hong Kong developer; {tokens} are interpolated via fill().
//
// Two vocabulary rules bind this module. A *Limit* is a rolling window with a
// ceiling that a Source's vendor imposes on the subscription (CONTEXT.md), and
// the synonyms that glossary tells us to avoid are avoided here too — no "rate
// limit", "quota", "allowance", "cap", or "throttle", in copy or in a comment.
// The copy says "window", which is the domain word. And no string
// here may name an operating system or one of its features: the same copy is
// read on every platform, and the credential this page discloses lives in a
// system keystore on one and a file on the others. The neutral phrasing below is
// what the spec's copy becomes once that rule is applied (strings.test.ts pins
// the first half of it).
export const limits = {
  en: {
    'limits.title': 'Limits',
    'limits.note':
      'Vendor windows, not your spend · checked when you open this page or press Refresh',
    'limits.refresh': 'Refresh',
    'limits.refreshing': 'Checking…',
    'limits.mode.left': 'Left',
    'limits.mode.used': 'Used',
    'limits.usageReset.one': '{n} Usage Reset',
    'limits.usageReset.many': '{n} Usage Resets',
    'limits.usageReset.a11yOne': '{n} Usage Reset available',
    'limits.usageReset.a11yMany': '{n} Usage Resets available',

    // Window labels. `five_hour`/`seven_day` are the two known Claude keys and
    // Codex classifies by duration; per-model windows are discovered from the
    // response's own `seven_day_*` keys, never a fixed list.
    'limits.win.session': 'Session',
    'limits.win.weekly': 'Weekly',
    'limits.win.weeklySub': 'Weekly',
    'limits.win.other': '{n} window',
    // Grok's bar meters a shared credit pool rather than a rate-limit window:
    // same geometry, different quantity, so the label says which (#126).
    'limits.win.weeklyCredits': 'Weekly credits',
    'limits.win.monthlyCredits': 'Monthly credits',

    // Pools. Antigravity meters two shared pools over the same two durations,
    // so the pool is part of the row label rather than a second card.
    'limits.pool.gemini': 'Gemini',
    'limits.pool.other': 'Other models',

    'limits.pctLeft': '{pct}% left',
    'limits.pctUsed': '{pct}% used',
    'limits.resetsIn': 'Resets in {t}',
    'limits.tickTitle': 'now — {t} until reset',
    'limits.spent': 'used up · resets in {t}',

    'limits.checkedNow': 'checked just now',
    'limits.checkedAgo': 'checked {t} ago',
    'limits.observedAgo': 'from your logs · last request {t} ago',
    'limits.observedOld': 'no requests in {t} — figures are that old',

    'limits.signedOut': 'Not signed in',
    'limits.signedOutHint': 'Sign in with the {cli} CLI, then check again.',
    'limits.checkAgain': 'Check again',
    'limits.error': "Couldn't check",
    'limits.retry': 'Retry',
    'limits.nothingRecorded': 'No {label} activity recorded yet',
    'limits.nothingRecordedHint': 'Readings appear the first time a scan finds a request.',

    // The opt-in empty state is the disclosure surface: it states exactly what
    // enabling does before any credential is read.
    'limits.optinTitle': 'See how much of your plan is left',
    // Tool names stay general rather than listed: the card grid directly below
    // already names each tool by icon and label, and a list would rot.
    'limits.optinBody':
      "Each card shows a vendor window — how much of it is used and when it resets. Checking live keeps them current: TokenLedger reads the sign-ins your AI tools already store for you and asks each vendor — read-only — how much of each window you've used.",
    // The old promise ("never changed, refreshed, or sent anywhere else") could
    // not survive the Google exchange (ADR-0020), so it is reframed to what is
    // actually true: the *saved* sign-in is untouched, and the disposable pass
    // the vendor's own client mints constantly is used once and never kept.
    'limits.optinBounds':
      'Only when you open this page or press Refresh — never on a timer. Your saved sign-in is never changed, and never sent anywhere but the vendor it belongs to. Some tools need a fresh access pass first; TokenLedger gets one the way the tool itself does, uses it once, and never keeps it.',
    'limits.optinButton': 'Enable live limit checks',

    'limits.t.d': '{n}d',
    'limits.t.h': '{n}h',
    'limits.t.m': '{n}m',
  },
  'zh-Hant': {
    'limits.title': '限額',
    'limits.note': '供應商的用量窗口，並非你的花費 · 開啟此頁或按「重新查詢」時才查詢',
    'limits.refresh': '重新查詢',
    'limits.refreshing': '查詢中…',
    'limits.mode.left': '剩餘',
    'limits.mode.used': '已用',
    'limits.usageReset.one': '{n} 次用量重置',
    'limits.usageReset.many': '{n} 次用量重置',
    'limits.usageReset.a11yOne': '可用的用量重置：{n} 次',
    'limits.usageReset.a11yMany': '可用的用量重置：{n} 次',

    'limits.win.session': '時段',
    'limits.win.weekly': '每週',
    'limits.win.weeklySub': '每週',
    'limits.win.other': '{n} 窗口',
    'limits.win.weeklyCredits': '每週額度',
    'limits.win.monthlyCredits': '每月額度',

    'limits.pool.gemini': 'Gemini',
    'limits.pool.other': '其他模型',

    'limits.pctLeft': '剩 {pct}%',
    'limits.pctUsed': '已用 {pct}%',
    'limits.resetsIn': '{t}後重置',
    'limits.tickTitle': '現在 — 距重置還有 {t}',
    'limits.spent': '已用盡 · {t}後重置',

    'limits.checkedNow': '剛剛查詢',
    'limits.checkedAgo': '{t}前查詢',
    'limits.observedAgo': '來自本機日誌 · 最後請求於 {t}前',
    'limits.observedOld': '{t}沒有請求 — 數字也是那時的',

    'limits.signedOut': '未登入',
    'limits.signedOutHint': '請先在 {cli} CLI 登入，再查詢一次。',
    'limits.checkAgain': '再查詢',
    'limits.error': '查詢失敗',
    'limits.retry': '重試',
    'limits.nothingRecorded': '尚未記錄到 {label} 的活動',
    'limits.nothingRecordedHint': '掃描第一次找到請求時就會出現讀數。',

    'limits.optinTitle': '看看方案還剩多少',
    'limits.optinBody':
      '每張卡是一個供應商窗口——用了多少、幾時重置。即時查詢讓數字保持最新：TokenLedger 會讀取你的 AI 工具已為你儲存的登入，以唯讀方式向各自的供應商查詢各窗口的使用量。',
    'limits.optinBounds':
      '只在開啟此頁或按「重新查詢」時查詢——絕不定時輪詢。你儲存的登入不會被修改，也不會傳送到它所屬供應商以外的任何地方。有些工具需要先換取臨時通行證；TokenLedger 會以該工具本身的做法換取一次，用完即棄，絕不保留。',
    'limits.optinButton': '啟用即時限額查詢',

    'limits.t.d': '{n} 天',
    'limits.t.h': '{n} 小時',
    'limits.t.m': '{n} 分鐘',
  },
};
