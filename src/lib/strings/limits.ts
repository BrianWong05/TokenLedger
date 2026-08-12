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

    // Window labels. `five_hour`/`seven_day` are the two known Claude keys and
    // Codex classifies by duration; per-model windows are discovered from the
    // response's own `seven_day_*` keys, never a fixed list.
    'limits.win.session': 'Session',
    'limits.win.weekly': 'Weekly',
    'limits.win.weeklySub': 'Weekly',
    'limits.win.other': '{n} window',

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
    'limits.optinBody':
      'Each card shows a vendor window — how much of it is used and when it resets. Codex is read from logs already on this computer. Claude needs a live check: TokenLedger reads the sign-in Claude Code already stores for you and asks Anthropic — read-only — how much of each window you have used.',
    'limits.optinBounds':
      'Only when you open this page or press Refresh — never on a timer. Your sign-in is never changed, refreshed, or sent anywhere else.',
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

    'limits.win.session': '時段',
    'limits.win.weekly': '每週',
    'limits.win.weeklySub': '每週',
    'limits.win.other': '{n} 窗口',

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
      '每張卡是一個供應商窗口——用了多少、幾時重置。Codex 直接讀這部機上已有的日誌。Claude 需要即時查詢：TokenLedger 會讀取 Claude Code 已為你儲存的登入，以唯讀方式向 Anthropic 查詢各窗口的使用量。',
    'limits.optinBounds':
      '只在開啟此頁或按「重新查詢」時查詢——絕不定時輪詢。你的登入不會被修改、續期或傳往其他地方。',
    'limits.optinButton': '啟用即時限額查詢',

    'limits.t.d': '{n} 天',
    'limits.t.h': '{n} 小時',
    'limits.t.m': '{n} 分鐘',
  },
};
