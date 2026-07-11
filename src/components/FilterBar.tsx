import type { Tool, DateRange, CustomRange, RangePreset } from '../types';

const TOOLS: Array<{ value: Tool | 'all'; label: string }> = [
  { value: 'all', label: 'All' },
  { value: 'claude', label: 'Claude' },
  { value: 'codex', label: 'Codex' },
  { value: 'gemini', label: 'Gemini' },
  { value: 'hermes', label: 'Hermes' },
  { value: 'grok', label: 'Grok' },
  { value: 'antigravity', label: 'Antigravity' },
];

const PRESETS: Array<{ value: RangePreset; label: string }> = [
  { value: 'today', label: 'Today' },
  { value: '7d', label: '7d' },
  { value: '30d', label: '30d' },
  { value: 'all', label: 'All' },
];

export interface FilterBarProps {
  tool: Tool | 'all';
  model: string | 'all';
  range: DateRange;
  refreshSec: 0 | 30 | 60;
  modelOptions: string[];
  onToolChange: (tool: Tool | 'all') => void;
  onModelChange: (model: string | 'all') => void;
  onRangeChange: (range: DateRange) => void;
  onRefreshChange: (sec: 0 | 30 | 60) => void;
}

function isCustom(range: DateRange): range is CustomRange {
  return typeof range === 'object';
}

function todayLocal(): string {
  const d = new Date();
  const m = String(d.getMonth() + 1).padStart(2, '0');
  const day = String(d.getDate()).padStart(2, '0');
  return `${d.getFullYear()}-${m}-${day}`;
}

export default function FilterBar({
  tool,
  model,
  range,
  refreshSec,
  modelOptions,
  onToolChange,
  onModelChange,
  onRangeChange,
  onRefreshChange,
}: FilterBarProps) {
  const custom = isCustom(range);
  const activePreset = custom ? 'custom' : range;

  return (
    <div className="filter-bar">
      <div className="segmented" role="group" aria-label="Tool">
        {TOOLS.map((t) => (
          <button
            key={t.value}
            type="button"
            className={tool === t.value ? 'seg active' : 'seg'}
            onClick={() => onToolChange(t.value)}
          >
            {t.label}
          </button>
        ))}
      </div>

      <select
        className="filter-select"
        aria-label="Model"
        value={model}
        onChange={(e) => onModelChange(e.target.value)}
      >
        <option value="all">All models</option>
        {modelOptions.map((m) => (
          <option key={m} value={m}>
            {m}
          </option>
        ))}
      </select>

      <div className="segmented" role="group" aria-label="Date range">
        {PRESETS.map((p) => (
          <button
            key={p.value}
            type="button"
            className={activePreset === p.value ? 'seg active' : 'seg'}
            onClick={() => onRangeChange(p.value)}
          >
            {p.label}
          </button>
        ))}
        <button
          type="button"
          className={activePreset === 'custom' ? 'seg active' : 'seg'}
          onClick={() =>
            onRangeChange(custom ? range : { start: todayLocal(), end: todayLocal() })
          }
        >
          Custom
        </button>
      </div>

      {custom && (
        <div className="custom-range">
          <input
            type="date"
            aria-label="Start date"
            value={range.start}
            max={range.end}
            onChange={(e) => onRangeChange({ ...range, start: e.target.value })}
          />
          <span className="range-sep">→</span>
          <input
            type="date"
            aria-label="End date"
            value={range.end}
            min={range.start}
            onChange={(e) => onRangeChange({ ...range, end: e.target.value })}
          />
        </div>
      )}

      <select
        className="filter-select"
        aria-label="Refresh interval"
        value={refreshSec}
        onChange={(e) => onRefreshChange(Number(e.target.value) as 0 | 30 | 60)}
      >
        <option value={0}>Refresh off</option>
        <option value={30}>Refresh 30s</option>
        <option value={60}>Refresh 60s</option>
      </select>
    </div>
  );
}
