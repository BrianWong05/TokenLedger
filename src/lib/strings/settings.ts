// Settings-tab strings. Owned by the Settings wave; extended freely here (and
// only here). Currency codes/names stay English in the dropdown values (ISO
// codes are universal); every section label and caption is translated.
export const settings = {
  en: {
    'settings.appearance': 'Appearance',
    'settings.theme': 'Theme',
    'settings.theme.caption': 'System follows macOS appearance',
    'settings.theme.system': 'System',
    'settings.theme.light': 'Light',
    'settings.theme.dark': 'Dark',
    'settings.language': 'Language',
    'settings.language.caption': 'English or 繁體中文',

    'settings.currencySection': 'Display currency',
    'settings.currency': 'Currency',
    'settings.currency.caption': 'Est. costs are converted for display only',
    'settings.rate': 'Exchange rate',
    'settings.rate.caption': 'Fixed rate you set — never fetched. Stored data stays USD.',

    'settings.startup': 'Startup',
    'settings.launch': 'Launch at login',
    'settings.launch.caption': 'Keeps recording before tools delete their logs (~30 days)',

    'settings.scanning': 'Scanning',
    'settings.refresh': 'Auto-refresh interval',
    'settings.refresh.caption': 'How often usage data is re-read from disk',
    'settings.refresh.custom': 'Custom',
    'settings.refreshCustom': 'Custom interval',
    'settings.refreshCustom.caption': 'Any whole number of seconds, 5 s – 24 h',
    'settings.refreshCustom.unit': 'seconds',

    'settings.updates': 'Updates',
    'settings.autoCheck': 'Check for updates automatically',
    'settings.autoCheck.caption': 'Once a day, in the background',
    'settings.checkNow': 'Check for updates',
    'settings.version': 'Version',
    'settings.updates.unconfigured': 'Update checks arrive with signed releases',
    'settings.updates.upToDate': 'Up to date',
    'settings.updates.isReady': 'is ready',
    'settings.updates.downloadedBg': 'Downloaded in the background',
    'settings.updates.releaseNotes': 'Release notes',
    'settings.updates.restart': 'Restart to update',
    'settings.updates.downloadedNote': 'downloaded · restart to install',
    'settings.updates.availableNote': 'available',

    'settings.footer': 'TokenLedger only reads local log files. Nothing leaves this Mac.',

    'settings.firstRun.title': 'TokenLedger keeps recording in the background',
    'settings.firstRun.body':
      'Coding tools delete their local logs after about 30 days. TokenLedger starts at login and scans quietly, so your usage is saved before it disappears. Everything stays on this Mac.',
    'settings.firstRun.launchCaption': 'Change anytime in Settings → Startup',
    'settings.firstRun.footnote': 'Scans are local file reads — nothing is uploaded.',
    'settings.firstRun.ok': 'OK',
  },
  'zh-Hant': {
    'settings.appearance': '外觀',
    'settings.theme': '主題',
    'settings.theme.caption': '「系統」會跟隨 macOS 外觀',
    'settings.theme.system': '系統',
    'settings.theme.light': '淺色',
    'settings.theme.dark': '深色',
    'settings.language': '語言',
    'settings.language.caption': 'English 或 繁體中文',

    'settings.currencySection': '顯示貨幣',
    'settings.currency': '貨幣',
    'settings.currency.caption': '預估成本僅為顯示而換算',
    'settings.rate': '匯率',
    'settings.rate.caption': '你自訂的固定匯率 — 不會抓取。儲存的資料維持美元。',

    'settings.startup': '啟動',
    'settings.launch': '登入時啟動',
    'settings.launch.caption': '在工具刪除記錄檔（約 30 天）之前持續記錄',

    'settings.scanning': '掃描',
    'settings.refresh': '自動重新整理間隔',
    'settings.refresh.caption': '多久從磁碟重新讀取一次使用資料',
    'settings.refresh.custom': '自訂',
    'settings.refreshCustom': '自訂間隔',
    'settings.refreshCustom.caption': '任意整數秒數，5 秒至 24 小時',
    'settings.refreshCustom.unit': '秒',

    'settings.updates': '更新',
    'settings.autoCheck': '自動檢查更新',
    'settings.autoCheck.caption': '每天一次，在背景執行',
    'settings.checkNow': '檢查更新',
    'settings.version': '版本',
    'settings.updates.unconfigured': '簽署版本推出後即可檢查更新',
    'settings.updates.upToDate': '已是最新版本',
    'settings.updates.isReady': '已就緒',
    'settings.updates.downloadedBg': '已在背景下載',
    'settings.updates.releaseNotes': '版本說明',
    'settings.updates.restart': '重新啟動以更新',
    'settings.updates.downloadedNote': '已下載 · 重新啟動以安裝',
    'settings.updates.availableNote': '可更新',

    'settings.footer': 'TokenLedger 只會讀取本機的記錄檔。沒有任何資料離開這台 Mac。',

    'settings.firstRun.title': 'TokenLedger 會在背景持續記錄',
    'settings.firstRun.body':
      '編碼工具約 30 天後就會刪除本機記錄檔。TokenLedger 會在登入時啟動並在背景靜默掃描，讓你的用量在消失前先被保存。所有資料都留在這台 Mac。',
    'settings.firstRun.launchCaption': '隨時可在「設定 → 啟動」變更',
    'settings.firstRun.footnote': '掃描只是本機檔案讀取 — 不會上傳任何東西。',
    'settings.firstRun.ok': '確定',
  },
};
