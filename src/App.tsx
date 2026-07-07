import { useCallback, useEffect, useMemo, useState } from 'react';
import type {
  Tool,
  DateRange,
  Filters,
  Summary,
  TrendPoint,
  BreakdownRow,
} from './types';
import { scan, fetchSummary, fetchTrend, fetchBreakdown } from './api';
import { rangeToBounds } from './lib/dateRange';
import { formatTokens, formatCost } from './lib/format';

export default function App() {
  const [tool, setTool] = useState<Tool | 'all'>('all');
  const [model, setModel] = useState<string | 'all'>('all');
  const [range, setRange] = useState<DateRange>('30d');
  const [refreshSec, setRefreshSec] = useState<0 | 30 | 60>(30);

  const [summary, setSummary] = useState<Summary | null>(null);
  const [trend, setTrend] = useState<TrendPoint[]>([]);
  const [modelRows, setModelRows] = useState<BreakdownRow[]>([]);
  const [projectRows, setProjectRows] = useState<BreakdownRow[]>([]);

  const [loading, setLoading] = useState(false);
  const [scanning, setScanning] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const filters: Filters = useMemo(() => {
    const { startTs, endTs } = rangeToBounds(range);
    return {
      tools: tool === 'all' ? [] : [tool],
      models: model === 'all' ? [] : [model],
      project: null,
      startTs,
      endTs,
    };
  }, [tool, model, range]);

  const loadData = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const bucket = range === 'today' ? 'hour' : 'day';
      const [s, t, m, p] = await Promise.all([
        fetchSummary(filters),
        fetchTrend(filters, bucket),
        fetchBreakdown('model', filters),
        fetchBreakdown('project', filters),
      ]);
      setSummary(s);
      setTrend(t);
      setModelRows(m);
      setProjectRows(p);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, [filters, range]);

  const runScan = useCallback(async () => {
    setScanning(true);
    try {
      await scan();
      await loadData();
    } catch (e) {
      setError(String(e));
    } finally {
      setScanning(false);
    }
  }, [loadData]);

  // Scan once on mount.
  useEffect(() => {
    runScan();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Refetch whenever the active filters change.
  useEffect(() => {
    loadData();
  }, [loadData]);

  // Auto-refresh timer; 0 = off. Cleared and recreated on interval change.
  useEffect(() => {
    if (refreshSec === 0) return;
    const id = setInterval(() => {
      runScan();
    }, refreshSec * 1000);
    return () => clearInterval(id);
  }, [refreshSec, runScan]);

  const modelOptions = modelRows.map((r) => r.key);

  return (
    <div className="app">
      <h1>TokenLedger</h1>

      <div style={{ display: 'flex', gap: 12, marginBottom: 16 }}>
        <select
          value={tool}
          onChange={(e) => setTool(e.target.value as Tool | 'all')}
        >
          <option value="all">All tools</option>
          <option value="claude">Claude</option>
          <option value="codex">Codex</option>
          <option value="gemini">Gemini</option>
          <option value="hermes">Hermes</option>
        </select>

        <select value={model} onChange={(e) => setModel(e.target.value)}>
          <option value="all">All models</option>
          {modelOptions.map((m) => (
            <option key={m} value={m}>
              {m}
            </option>
          ))}
        </select>

        <select
          value={typeof range === 'string' ? range : 'custom'}
          onChange={(e) => setRange(e.target.value as DateRange)}
        >
          <option value="today">Today</option>
          <option value="7d">7d</option>
          <option value="30d">30d</option>
          <option value="all">All</option>
        </select>

        <select
          value={refreshSec}
          onChange={(e) =>
            setRefreshSec(Number(e.target.value) as 0 | 30 | 60)
          }
        >
          <option value={0}>Off</option>
          <option value={30}>30s</option>
          <option value={60}>60s</option>
        </select>

        <button onClick={runScan}>Rescan</button>
      </div>

      {error && <div style={{ color: '#ff5c7a' }}>Error: {error}</div>}
      {loading && <div>Loading…</div>}

      {summary && (
        <div style={{ marginBottom: 16 }}>
          <div style={{ fontSize: 40, fontWeight: 700 }}>
            {formatTokens(summary.totalTokens)}
          </div>
          <div>total tokens</div>
          <div>{summary.requests.toLocaleString('en-US')} requests</div>
          <div>{formatCost(summary.cost, summary.hasUnpriced)}</div>
          <div>at API list prices — not billed</div>
          <div>input {formatTokens(summary.inputTokens)}</div>
          <div>output {formatTokens(summary.outputTokens)}</div>
          <div>cache write {formatTokens(summary.cacheWriteTokens)}</div>
          <div>cache read {formatTokens(summary.cacheReadTokens)}</div>
          <div>
            cache hit rate {(summary.cacheHitRate * 100).toFixed(1)}%
          </div>
        </div>
      )}

      <div>trend points: {trend.length}</div>

      <h2>By model</h2>
      <table>
        <tbody>
          {modelRows.map((r) => (
            <tr key={r.key}>
              <td>{r.key}</td>
              <td>{formatTokens(r.totalTokens)}</td>
              <td>{r.requests}</td>
              <td>{formatCost(r.cost, false)}</td>
            </tr>
          ))}
        </tbody>
      </table>

      <h2>By project</h2>
      <table>
        <tbody>
          {projectRows.map((r) => (
            <tr key={r.key}>
              <td>{r.key}</td>
              <td>{formatTokens(r.totalTokens)}</td>
              <td>{r.requests}</td>
              <td>{formatCost(r.cost, false)}</td>
            </tr>
          ))}
        </tbody>
      </table>

      <div style={{ marginTop: 16, color: '#8b8b96' }}>
        {scanning ? 'scanning…' : 'idle'}
      </div>
    </div>
  );
}
