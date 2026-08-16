// Settings-tab strings. Owned by the Settings wave; extended freely here (and
// only here). Currency codes/names stay English in the dropdown values (ISO
// codes are universal); every section label and caption is translated.
export const settings = {
  en: {
    'settings.appearance': 'Appearance',
    'settings.theme': 'Theme',
    'settings.theme.caption': 'System follows your OS appearance',
    'settings.theme.system': 'System',
    'settings.theme.light': 'Light',
    'settings.theme.dark': 'Dark',
    'settings.language': 'Language',
    'settings.customRange': 'Custom range',
    // "Preset" is the glossary's word for these (CONTEXT.md); "shortcut" stays
    // informal prose for comments, not UI.
    'settings.preset.add': 'Add a preset',
    // "four" is MAX_CUSTOM_PRESETS (customPresets.ts) — the picker's column
    // holds four configured presets beside its four shipped ones.
    'settings.preset.caption': 'Up to four extra presets in the Custom range picker',
    // The calendar periods themselves are named once, in the overview catalog —
    // this control shows the same words the picker will.
    'settings.preset.type': 'Preset type',
    'settings.preset.rolling': 'Last N days',
    'settings.preset.dayCount': 'Day count',
    'settings.preset.daysUnit': 'days',
    'settings.preset.addAction': 'Add',
    'settings.preset.remove': 'Remove',
    // The picker lists configured presets in this order, so these move a preset
    // past the one above or below it there too.
    'settings.preset.moveUp': 'Move up',
    'settings.preset.moveDown': 'Move down',
    'settings.preset.none': 'None yet — the picker shows its four built-in presets',
    // A period that ends before the first record: the picker does not offer it
    // at all, so the row says why rather than naming a window nothing can pick.
    'settings.preset.outside': 'Ends before your first record — not offered yet',
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
    'settings.refresh.off': 'Off',
    // "Off" on an app whose job is recording has to say what it does not stop.
    'settings.refresh.offNote':
      'This window re-reads only when you press Rescan. Background recording carries on.',
    'settings.refresh.custom': 'Custom',
    'settings.refreshCustom': 'Custom interval',
    'settings.refreshCustom.caption': 'Any whole number of seconds, 5 s – 24 h',
    'settings.refreshCustom.unit': 'seconds',

    // The glossary's name for this presence on every platform, which is why it
    // is not "menu bar": that names the macOS place, and the same section ships
    // where the place is a Windows notification area or a Linux system tray.
    'settings.menuBar': 'Menu Bar Extra',
    'settings.menuBarRefresh': 'Refresh interval',
    // Beside "Auto-refresh interval" the short title is ambiguous once the
    // section heading is out of reach, so the control carries the long name.
    'settings.menuBarRefresh.aria': 'Menu Bar Extra refresh interval',
    // The distinction from the row above: that one keeps a window you are
    // looking at current, this one keeps the figures beside the icon current
    // when there is no window at all.
    'settings.menuBarRefresh.caption':
      "How often the Menu Bar Extra's figures are re-read while every window is closed",
    'settings.menuBarRefresh.off': 'Off',
    // Same rule as the auto-refresh note: "Off" has to say what it does not
    // stop. Here it paces the figures back to the resident cadence, nothing more.
    'settings.menuBarRefresh.offNote':
      'The Menu Bar Extra falls back to a scan every few hours. Recording never stops.',

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

    'settings.footer': 'TokenLedger only reads local log files. Nothing leaves this computer.',

    'settings.firstRun.title': 'TokenLedger keeps recording in the background',
    'settings.firstRun.body':
      'Coding tools delete their local logs after about 30 days. TokenLedger starts at login and scans quietly, so your usage is saved before it disappears. Everything stays on this computer.',
    'settings.firstRun.launchCaption': 'Change anytime in Settings → Startup',
    // The second sentence is the honest half: one optional feature does reach a
    // vendor, and burying that here would be the wrong kind of quiet.
    'settings.firstRun.footnote':
      'Scans are local file reads — nothing is uploaded. Live limit checks are separate, optional, and asked about on the Limits tab before anything runs.',
    'settings.firstRun.ok': 'OK',
  },
  'zh-Hant': {
    'settings.appearance': '外觀',
    'settings.theme': '主題',
    'settings.theme.caption': '「系統」會跟隨作業系統外觀',
    'settings.theme.system': '系統',
    'settings.theme.light': '淺色',
    'settings.theme.dark': '深色',
    'settings.language': '語言',
    'settings.customRange': '自訂範圍',
    // "Preset" stays 快捷範圍 (quick range) rather than the literal 預設範圍:
    // 預設 reads as "default", and these are not defaults.
    'settings.preset.add': '新增快捷範圍',
    'settings.preset.caption': '自訂範圍選擇器中最多四個額外快捷範圍',
    'settings.preset.type': '快捷範圍類型',
    'settings.preset.rolling': '過去 N 天',
    'settings.preset.dayCount': '天數',
    'settings.preset.daysUnit': '天',
    'settings.preset.addAction': '新增',
    'settings.preset.remove': '移除',
    'settings.preset.moveUp': '上移',
    'settings.preset.moveDown': '下移',
    'settings.preset.none': '尚未新增 — 選擇器只顯示內建的四個快捷範圍',
    'settings.preset.outside': '結束於首筆記錄之前 — 選擇器尚未提供',
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
    'settings.refresh.off': '關閉',
    'settings.refresh.offNote': '此視窗只在你按「重新掃描」時重讀。背景記錄照常進行。',
    'settings.refresh.custom': '自訂',
    'settings.refreshCustom': '自訂間隔',
    'settings.refreshCustom.caption': '任意整數秒數，5 秒至 24 小時',
    'settings.refreshCustom.unit': '秒',

    'settings.menuBar': '選單列輔助程式',
    'settings.menuBarRefresh': '重新整理間隔',
    'settings.menuBarRefresh.aria': '選單列輔助程式重新整理間隔',
    'settings.menuBarRefresh.caption': '所有視窗關閉時，選單列輔助程式的數字多久重讀一次',
    'settings.menuBarRefresh.off': '關閉',
    'settings.menuBarRefresh.offNote': '選單列輔助程式改回每幾小時掃描一次。記錄不會停止。',

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

    'settings.footer': 'TokenLedger 只會讀取本機的記錄檔。沒有任何資料離開這台電腦。',

    'settings.firstRun.title': 'TokenLedger 會在背景持續記錄',
    'settings.firstRun.body':
      '編碼工具約 30 天後就會刪除本機記錄檔。TokenLedger 會在登入時啟動並在背景靜默掃描，讓你的用量在消失前先被保存。所有資料都留在這台電腦。',
    'settings.firstRun.launchCaption': '隨時可在「設定 → 啟動」變更',
    'settings.firstRun.footnote':
      '掃描只是本機檔案讀取 — 不會上傳任何東西。即時限額查詢是另一回事：可選，且會在「限額」分頁先徵求同意才執行。',
    'settings.firstRun.ok': '確定',
  },
};
