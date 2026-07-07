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
import FilterBar from './components/FilterBar';
import HeroCard from './components/HeroCard';
import StatCards from './components/StatCards';

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

      <FilterBar
        tool={tool}
        model={model}
        range={range}
        refreshSec={refreshSec}
        modelOptions={modelOptions}
        onToolChange={setTool}
        onModelChange={setModel}
        onRangeChange={setRange}
        onRefreshChange={setRefreshSec}
      />
      <button onClick={runScan}>Rescan</button>

      {error && <div style={{ color: '#ff5c7a' }}>Error: {error}</div>}
      {loading && <div>Loading…</div>}

      <HeroCard summary={summary} />
      <StatCards summary={summary} />

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
