// Overview-tab strings. Owned by the Overview-retrofit wave; en is the key
// universe, zh-Hant is natural Traditional Chinese for a Hong Kong developer.
// Product/Source names (Claude Code, Codex…), raw model names, ISO codes, file
// paths, and numeric units (K/M/B, %, 2D/3D, tokens) are NOT translated. Counts
// are interpolated in the components (English pluralises with a one/many key).
export const overview = {
  en: {
    // header + headline
    'overview.title': 'Overview',
    'overview.customRange': 'Custom range',
    'overview.totalTokens': 'Total tokens',
    // short visible note; the full "not billed" caveat rides as a title tooltip
    // (overview.notBilled) so the ADR-0002 honesty survives the shortening
    'overview.costNote': 'est.',
    'overview.unpricedMarker': 'unpriced',
    'overview.cacheEst': 'cache est.',
    'overview.showCostBreakdown': 'Show Cost breakdown',
    'overview.modelOne': 'model',
    'overview.modelMany': 'models',

    // range presets (segment labels + eyebrow long forms)
    'overview.range.day': 'Day',
    'overview.range.week': 'Week',
    'overview.range.month': 'Month',
    'overview.range.total': 'Total',
    'overview.range.custom': 'Custom',
    'overview.range.day.long': 'Today',
    'overview.range.week.long': 'Last 7 days',
    'overview.range.month.long': 'Last 30 days',
    'overview.range.total.long': 'All time',
    'overview.range.custom.long': 'Custom range',

    // custom-range picker — shortcuts, calendar chrome, and the picking hints.
    // No shortcut repeats a segment (Week = trailing 7 days, Month = trailing
    // 30, Total = everything), so "This month" means the calendar month.
    'overview.preset.yesterday': 'Yesterday',
    'overview.preset.thisMonth': 'This month',
    'overview.preset.last90': 'Last 90 days',
    'overview.preset.thisYear': 'This year',
    // configured shortcuts (Settings -> Custom range). The rolling one has no
    // static label — an arbitrary N is composed from this prefix and the day
    // count, which is how the picker's own span hint already reads.
    'overview.preset.lastMonth': 'Last month',
    'overview.preset.lastQuarter': 'Last quarter',
    'overview.preset.lastYear': 'Last year',
    'overview.preset.lastN': 'Last',
    'overview.pickStart': 'Pick the start date',
    'overview.pickEnd': 'Pick the end date',
    'overview.prevMonth': 'Previous month',
    'overview.nextMonth': 'Next month',
    'overview.month': 'Month',
    'overview.year': 'Year',
    'overview.dayOne': 'day',

    // heatmap
    'overview.activity': 'Activity',
    'overview.hoverDay': 'hover a day',
    'overview.fullYearScroll': 'full year · scroll ⟷ · hover a day',
    'overview.tokens': 'tokens',
    'overview.noActivity': 'No activity',
    'overview.activeDays': 'active days',
    'overview.dayStreak': 'day streak',
    'overview.best': 'best',
    'overview.heatLess': 'Less',
    'overview.heatMore': 'More',

    // activity enlarge (full-screen 3D landscape)
    'overview.enlarge': 'Enlarge',
    'overview.close': 'Close',
    'overview.insight3d': '3D Insight',
    'overview.token3dPerspective': 'Token 3D perspective',
    'overview.resetView': 'Reset view',
    'overview.zoomRotate': 'Scroll to zoom · drag to rotate',
    'overview.estCost': 'Est. cost',
    'overview.longestStreak': 'Longest streak',
    'overview.peakDay': 'Peak day',
    'overview.daysUnit': 'days',

    // trend + small multiples
    'overview.usageTrend': 'Usage over time',
    'overview.stackedByTool': 'Stacked by tool',
    'overview.total': 'total',
    'overview.modelBreakdown': 'Model breakdown',
    'overview.more': 'more',
    'overview.avg': 'avg',
    'overview.per.hour': 'hour',
    'overview.per.day': 'day',
    'overview.per.week': 'week',
    'overview.per.month': 'month',
    'overview.peak': 'peak',
    'overview.perToolTrend': 'Per-tool trend',

    // usage-trend enlarge — bucket inspector (design 1b)
    'overview.trend.selDay': 'Selected day',
    'overview.trend.selHour': 'Selected hour',
    'overview.trend.selWeek': 'Selected week',
    'overview.trend.selMonth': 'Selected month',
    'overview.trend.vsAvg': 'vs avg',
    'overview.trend.byModel': 'By model',
    'overview.trend.moreModels': 'more models',
    'overview.moreSkills': 'more skills',
    'overview.trend.exportCsv': 'Export CSV',
    // the toolbar's window report: one CSV of whatever the Overview is showing
    'overview.export': 'Export',
    'overview.exporting': 'Exporting…',
    'overview.exportFailed': 'Export failed',
    // interval selector — adjective forms, so the options can't be confused
    // with the window presets (Day/Week/Month) sitting beside them
    'overview.trend.int.auto': 'Auto',
    'overview.trend.int.day': 'Daily',
    'overview.trend.int.week': 'Weekly',
    'overview.trend.int.month': 'Monthly',

    // token breakdown
    'overview.tokenBreakdown': 'Token Breakdown',
    'overview.cacheHitRate': 'Cache hit rate',
    'overview.reused': 'reused',
    'overview.cat.input': 'Input',
    'overview.cat.output': 'Output',
    'overview.cat.cacheRead': 'Cache read',
    'overview.cat.cacheWrite': 'Cache write',

    // context breakdown
    'overview.contextBreakdown': 'Context Breakdown',
    'overview.ctxInputWord': 'input',
    'overview.messages': 'Messages',
    'overview.convHistory': 'Conversation history',
    'overview.newInput': 'New input',
    'overview.assistantResponse': 'Assistant response',
    'overview.systemPrompt': 'System prompt',
    'overview.reasoning': 'Reasoning',
    'overview.toolCalls': 'Tool calls',
    'overview.customAgents': 'Custom agents',
    'overview.mcpServers': 'MCP servers',
    'overview.skills': 'Skills',
    'overview.exec.byType': 'By type',
    'overview.exec.executable': 'Executable',
    'overview.exec.command': 'Command',
    'overview.exec.type': 'Type',
    'overview.exec.calls': 'Calls',
    'overview.exec.total': 'Total',
    // ctxMeta resource kinds
    'overview.kind.skill': 'skill',
    'overview.kind.mcpServer': 'MCP server',
    'overview.kind.agent': 'agent',
    'overview.kind.memoryFile': 'memory file',

    // breakdown table
    'overview.col.total': 'Total',
    'overview.col.input': 'Input',
    'overview.col.output': 'Output',
    'overview.col.cached': 'Cached',
    'overview.col.reasoning': 'Reasoning',
    'overview.col.convs': 'Convs',
    'overview.col.date': 'Date',
    'overview.col.project': 'Project',
    'overview.dailyBreakdown': 'Daily Breakdown',
    'overview.projectUsage': 'Project Usage',
    'overview.reasoningNote': 'Claude does not report reasoning separately',

    // scan footer
    'overview.scanIn': 'in',
    'overview.scanSkipped': 'skipped',
    'overview.scanUnreadable': 'unreadable',
    'overview.scanUnbooked': 'unbooked',

    // Grok Build new-telemetry heads-up: informational, never a Source in trouble
    'overview.scanNoticeOne': 'unrecognized log line skipped',
    'overview.scanNoticeMany': 'unrecognized log lines skipped',

    // Requests a Source reports no tokens for (TOKL-25): no Usage Record can
    // exist for them, so the count is all there is to say. Informational for
    // the same reason as the notice above — nobody can make the Source log
    // figures it does not have — and it names Requests, never tokens.
    'overview.unbookedNoticeOne': 'Request reports no tokens — not booked',
    'overview.unbookedNoticeMany': 'Requests report no tokens — not booked',

    // Unreadable Artifacts (ADR-0017): the ≥ reason and its aria reading
    'overview.unreadableSessionOne': 'session unreadable',
    'overview.unreadableSessionMany': 'sessions unreadable',
    'overview.atLeast': 'at least',
    // The export companion (ADR-0018): offered only where the ≥ is explained,
    // because it is the one action that removes it.
    'overview.decrypt': 'Decrypt',
    'overview.decrypting': 'Decrypting…',
    'overview.decryptHint': 'Read the encrypted Sessions using Antigravity, which must be running',

    // models list
    'overview.modelsHead': 'Models',

    // profile (the window's Models across every Source; footer is the Ledger's)
    'overview.profile.started': 'First record',
    'overview.profile.activeDays': 'Active days',
    'overview.profile.empty': 'No usage in this window',
    'overview.profile.unattributedOnly': 'Only Unattributed usage in this window',
    'overview.profile.showAll': 'Show all {n}',
    'overview.profile.showTop': 'Show top {n}',
    'overview.profile.shareTitle': "Share of this window's tokens, including Unattributed usage",

    // cost breakdown modal + cost markers
    'overview.estTotalCost': 'Estimated total Cost',
    'overview.notBilled': 'At API list prices — not billed',
    'overview.closeCostBreakdown': 'Close Cost breakdown',
    'overview.col.model': 'Model',
    'overview.col.cost': 'Cost',
    'overview.cacheEstimated': 'Cache-Estimated',
    'overview.unpricedLabel': 'Unpriced',
    'overview.unavailableCost': 'Unavailable',
    'overview.unattributedUsage': 'Unattributed usage',
    'overview.partialCost': 'Partial Cost',
    'overview.unpricedModelOne': 'Unpriced Model',
    'overview.unpricedModelMany': 'Unpriced Models',

    // token total headline
    'overview.showCompact': 'Show compact token count',
    'overview.showExact': 'Show exact token count',
    'overview.totalTokensAria': 'total tokens.',
  },
  'zh-Hant': {
    'overview.title': '總覽',
    'overview.customRange': '自訂範圍',
    'overview.totalTokens': '總 token 數',
    'overview.costNote': '估算',
    'overview.unpricedMarker': '未定價',
    'overview.cacheEst': '快取估算',
    'overview.showCostBreakdown': '顯示成本明細',
    'overview.modelOne': '個模型',
    'overview.modelMany': '個模型',

    'overview.range.day': '日',
    'overview.range.week': '週',
    'overview.range.month': '月',
    'overview.range.total': '全部',
    'overview.range.custom': '自訂',
    'overview.range.day.long': '今天',
    'overview.range.week.long': '過去 7 天',
    'overview.range.month.long': '過去 30 天',
    'overview.range.total.long': '全部時間',
    'overview.range.custom.long': '自訂範圍',

    'overview.preset.yesterday': '昨天',
    'overview.preset.thisMonth': '本月',
    'overview.preset.last90': '過去 90 天',
    'overview.preset.thisYear': '今年',
    'overview.preset.lastMonth': '上個月',
    'overview.preset.lastQuarter': '上一季',
    'overview.preset.lastYear': '去年',
    'overview.preset.lastN': '過去',
    'overview.pickStart': '選擇開始日期',
    'overview.pickEnd': '選擇結束日期',
    'overview.prevMonth': '上個月',
    'overview.nextMonth': '下個月',
    'overview.month': '月份',
    'overview.year': '年份',
    'overview.dayOne': '天',

    'overview.activity': '活動',
    'overview.hoverDay': '將游標移到日期上',
    'overview.fullYearScroll': '全年 · 左右捲動 · 將游標移到日期上',
    'overview.tokens': 'tokens',
    'overview.noActivity': '沒有活動',
    'overview.activeDays': '活躍天數',
    'overview.dayStreak': '連續天數',
    'overview.best': '最高',
    'overview.heatLess': '較少',
    'overview.heatMore': '較多',

    // activity enlarge (full-screen 3D landscape)
    'overview.enlarge': '放大',
    'overview.close': '關閉',
    'overview.insight3d': '3D 洞察',
    'overview.token3dPerspective': 'Token 3D 透視',
    'overview.resetView': '重設視角',
    'overview.zoomRotate': '捲動縮放 · 拖曳旋轉',
    'overview.estCost': '預估成本',
    'overview.longestStreak': '最長連續',
    'overview.peakDay': '最高峰日',
    'overview.daysUnit': '天',

    'overview.usageTrend': '使用量趨勢',
    'overview.stackedByTool': '依工具堆疊',
    'overview.total': '總計',
    'overview.modelBreakdown': '模型明細',
    'overview.more': '更多',
    'overview.avg': '平均',
    'overview.per.hour': '小時',
    'overview.per.day': '日',
    'overview.per.week': '週',
    'overview.per.month': '月',
    'overview.peak': '尖峰',
    'overview.perToolTrend': '各工具趨勢',

    // usage-trend enlarge — bucket inspector (design 1b)
    'overview.trend.selDay': '所選日',
    'overview.trend.selHour': '所選時段',
    'overview.trend.selWeek': '所選週',
    'overview.trend.selMonth': '所選月',
    'overview.trend.vsAvg': '對比平均',
    'overview.trend.byModel': '依模型',
    'overview.trend.moreModels': '個其他模型',
    'overview.moreSkills': '個其他技能',
    'overview.trend.exportCsv': '匯出 CSV',
    // the toolbar's window report: one CSV of whatever the Overview is showing
    'overview.export': '匯出',
    'overview.exporting': '匯出中…',
    'overview.exportFailed': '匯出失敗',
    // interval selector — adjective forms, so the options can't be confused
    // with the window presets (Day/Week/Month) sitting beside them
    'overview.trend.int.auto': '自動',
    'overview.trend.int.day': '每日',
    'overview.trend.int.week': '每週',
    'overview.trend.int.month': '每月',

    'overview.tokenBreakdown': 'Token 明細',
    'overview.cacheHitRate': '快取命中率',
    'overview.reused': '重用',
    'overview.cat.input': '輸入',
    'overview.cat.output': '輸出',
    'overview.cat.cacheRead': '快取讀取',
    'overview.cat.cacheWrite': '快取寫入',

    'overview.contextBreakdown': '內容明細',
    'overview.ctxInputWord': '輸入',
    'overview.messages': '訊息',
    'overview.convHistory': '對話記錄',
    'overview.newInput': '新輸入',
    'overview.assistantResponse': '助手回應',
    'overview.systemPrompt': '系統提示',
    'overview.reasoning': '推理',
    'overview.toolCalls': '工具呼叫',
    'overview.customAgents': '自訂代理',
    'overview.mcpServers': 'MCP 伺服器',
    'overview.skills': '技能',
    'overview.exec.byType': '按類型',
    'overview.exec.executable': '可執行檔',
    'overview.exec.command': '指令',
    'overview.exec.type': '類型',
    'overview.exec.calls': '呼叫次數',
    'overview.exec.total': '總計',
    'overview.kind.skill': '技能',
    'overview.kind.mcpServer': 'MCP 伺服器',
    'overview.kind.agent': '代理',
    'overview.kind.memoryFile': '記憶檔案',

    'overview.col.total': '總計',
    'overview.col.input': '輸入',
    'overview.col.output': '輸出',
    'overview.col.cached': '快取',
    'overview.col.reasoning': '推理',
    'overview.col.convs': '對話',
    'overview.col.date': '日期',
    'overview.col.project': '專案',
    'overview.dailyBreakdown': '每日明細',
    'overview.projectUsage': '專案用量',
    'overview.reasoningNote': 'Claude 不會單獨回報推理',

    'overview.scanIn': '匯入',
    'overview.scanSkipped': '略過',
    'overview.scanUnreadable': '無法讀取',
    'overview.scanUnbooked': '未計入',

    'overview.scanNoticeOne': '行無法識別的記錄已略過',
    'overview.scanNoticeMany': '行無法識別的記錄已略過',

    'overview.unbookedNoticeOne': '個要求未回報 token — 未計入',
    'overview.unbookedNoticeMany': '個要求未回報 token — 未計入',

    'overview.unreadableSessionOne': '個工作階段無法讀取',
    'overview.unreadableSessionMany': '個工作階段無法讀取',
    'overview.atLeast': '至少',
    'overview.decrypt': '解密',
    'overview.decrypting': '解密中…',
    'overview.decryptHint': '透過 Antigravity 讀取加密的工作階段，需先開啟 Antigravity',

    'overview.modelsHead': '模型',

    'overview.profile.started': '首筆紀錄',
    'overview.profile.activeDays': '活躍天數',
    'overview.profile.empty': '此區間尚無用量',
    'overview.profile.unattributedOnly': '此區間只有無歸屬用量',
    'overview.profile.showAll': '顯示全部 {n} 個',
    'overview.profile.showTop': '只顯示前 {n} 個',
    'overview.profile.shareTitle': '佔此區間所有 token 的比例（含無歸屬用量）',

    'overview.estTotalCost': '預估總成本',
    'overview.notBilled': '以 API 列表價計算 — 並非實際帳單',
    'overview.closeCostBreakdown': '關閉成本明細',
    'overview.col.model': '模型',
    'overview.col.cost': '成本',
    'overview.cacheEstimated': '快取估算',
    'overview.unpricedLabel': '未定價',
    'overview.unavailableCost': '無法取得',
    'overview.unattributedUsage': '無法歸屬的用量',
    'overview.partialCost': '部分成本',
    'overview.unpricedModelOne': '個未定價模型',
    'overview.unpricedModelMany': '個未定價模型',

    'overview.showCompact': '顯示精簡 token 數',
    'overview.showExact': '顯示完整 token 數',
    'overview.totalTokensAria': '總 token 數。',
  },
};
