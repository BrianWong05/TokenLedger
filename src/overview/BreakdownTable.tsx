import { useMemo, useState } from 'react';
import { type TableRow } from './data';
import { fmtIsoDate } from '../lib/format';

type Tab = 'daily' | 'projects';
type SortKey = keyof TableRow;

const NUMCOLS: { key: SortKey; label: string }[] = [
  { key: 'total', label: 'Total' },
  { key: 'input', label: 'Input' },
  { key: 'output', label: 'Output' },
  { key: 'cached', label: 'Cached' },
  { key: 'reasoning', label: 'Reasoning' },
  { key: 'convs', label: 'Convs' },
];

const fmtInt = (n: number) => n.toLocaleString('en-US');

// Daily breakdown / project usage — a tabbed, click-to-sort table (design 8b).
export default function BreakdownTable({
  dailyRows,
  projectRows,
}: {
  dailyRows: TableRow[];
  projectRows: TableRow[];
}) {
  const [tab, setTab] = useState<Tab>('daily');
  const [sort, setSort] = useState<{ key: SortKey; dir: 'asc' | 'desc' }>({ key: 'label', dir: 'desc' });

  const rows = tab === 'daily' ? dailyRows : projectRows;

  const sorted = useMemo(() => {
    const { key, dir } = sort;
    const num = (v: number | null) => (v == null ? -1 : v); // '—' sorts below 0
    return [...rows].sort((a, b) => {
      const av = a[key];
      const bv = b[key];
      const cmp =
        typeof av === 'string'
          ? av.localeCompare(bv as string)
          : num(av as number | null) - num(bv as number | null);
      return dir === 'asc' ? cmp : -cmp;
    });
  }, [rows, sort]);

  function switchTab(t: Tab) {
    setTab(t);
    setSort(t === 'daily' ? { key: 'label', dir: 'desc' } : { key: 'total', dir: 'desc' });
  }
  function clickCol(key: SortKey) {
    setSort((s) =>
      s.key === key
        ? { key, dir: s.dir === 'asc' ? 'desc' : 'asc' }
        : { key, dir: key === 'label' ? 'asc' : 'desc' },
    );
  }

  const cols: { key: SortKey; label: string }[] = [
    { key: 'label', label: tab === 'daily' ? 'Date' : 'Project' },
    ...NUMCOLS,
  ];

  return (
    <div className="tt-tbl">
      <div className="tt-tbl-tabs">
        <button className={tab === 'daily' ? 'active' : ''} onClick={() => switchTab('daily')}>
          Daily Breakdown
        </button>
        <button className={tab === 'projects' ? 'active' : ''} onClick={() => switchTab('projects')}>
          Project Usage
        </button>
      </div>
      <div className="tt-tbl-scroll">
        <div className="tt-tbl-grid tt-tbl-head">
          {cols.map((c) => (
            <button
              key={c.key}
              className={sort.key === c.key ? 'active' : ''}
              onClick={() => clickCol(c.key)}
              title={c.key === 'reasoning' ? 'Claude does not report reasoning separately' : undefined}
            >
              {c.label}
              <span className="arrow">{sort.key === c.key ? (sort.dir === 'asc' ? '▲' : '▼') : ''}</span>
            </button>
          ))}
        </div>
        {sorted.map((r, i) => (
          <div className="tt-tbl-grid tt-tbl-row" key={r.label + i}>
            <span>{tab === 'daily' ? fmtIsoDate(r.label) : r.label}</span>
            <span>{fmtInt(r.total)}</span>
            <span>{fmtInt(r.input)}</span>
            <span>{fmtInt(r.output)}</span>
            <span>{fmtInt(r.cached)}</span>
            <span>{r.reasoning == null ? '—' : fmtInt(r.reasoning)}</span>
            <span>{fmtInt(r.convs)}</span>
          </div>
        ))}
      </div>
    </div>
  );
}
