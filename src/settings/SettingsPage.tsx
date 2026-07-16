// Settings tab — placeholder. The shell mounts this and passes nothing; the
// Settings wave fills the container and pulls its own data through SettingsPort
// (src/settings/settings.ts), so it never edits the shell.
export default function SettingsPage() {
  return <div className="tl-page tl-page-settings" />;
}
