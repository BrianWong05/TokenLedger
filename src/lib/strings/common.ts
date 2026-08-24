// Shell / header / nav strings. Owned by the frontend-shell wave; the Overview,
// Pricing, and Settings retrofits keep their strings in their own modules so
// parallel waves never touch a shared file.
export const common = {
  en: {
    'nav.overview': 'Overview',
    'nav.pricing': 'Pricing',
    'nav.limits': 'Limits',
    'nav.settings': 'Settings',
    // The shell's update card says these itself rather than reading the
    // Settings dictionary — same words, but each module owns its own keys.
    'update.available': 'Update available',
    'update.ready': 'is ready',
    'update.staged': 'downloaded · restart to install',
    'update.action': 'Update',
    'update.downloading': 'Downloading…',
    'update.restart': 'Restart to update',
    // Not "Auto-updated": nothing installs itself here (updater.rs installs on
    // user approval only), so this announces the version it is running now,
    // however it got there.
    'update.applied': 'Updated',
    'update.dismiss': 'Dismiss',
    'header.rescan': 'Rescan',
    'header.scanning': 'Scanning…',
    'header.lastScan': 'last scan',
    'header.notScanned': 'not scanned yet',
  },
  'zh-Hant': {
    'nav.overview': '總覽',
    'nav.pricing': '價格',
    'nav.limits': '限額',
    'nav.settings': '設定',
    'update.available': '有可用更新',
    'update.ready': '已就緒',
    'update.staged': '已下載 · 重新啟動以安裝',
    'update.action': '更新',
    'update.downloading': '下載中…',
    'update.restart': '重新啟動以更新',
    'update.applied': '已更新',
    'update.dismiss': '關閉',
    'header.rescan': '重新掃描',
    'header.scanning': '掃描中…',
    'header.lastScan': '上次掃描',
    'header.notScanned': '尚未掃描',
  },
};
