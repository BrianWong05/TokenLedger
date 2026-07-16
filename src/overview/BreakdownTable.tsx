import { useMemo, useState } from 'react';
import { type TableRow } from './data';
import { fmtIsoDateL, useOverviewT, type OverviewKey } from './localize';

type Tab = 'daily' | 'projects';
type SortKey = keyof TableRow;

const NUMCOLS: { key: SortKey; labelKey: OverviewKey }[] = [
  { key: 'total', labelKey: 'overview.col.total' },
  { key: 'input', labelKey: 'overview.col.input' },
  { key: 'output', labelKey: 'overview.col.output' },
  { key: 'cached', labelKey: 'overview.col.cached' },
  { key: 'reasoning', labelKey: 'overview.col.reasoning' },
  { key: 'convs', labelKey: 'overview.col.convs' },
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
  const { t, lang } = useOverviewT();
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

  const cols: { key: SortKey; labelKey: OverviewKey }[] = [
    { key: 'label', labelKey: tab === 'daily' ? 'overview.col.date' : 'overview.col.project' },
    ...NUMCOLS,
  ];

  return (
    <div className="tt-tbl">
      <div className="tt-tbl-tabs">
        <button className={tab === 'daily' ? 'active' : ''} onClick={() => switchTab('daily')}>
          {t('overview.dailyBreakdown')}
        </button>
        <button className={tab === 'projects' ? 'active' : ''} onClick={() => switchTab('projects')}>
          {t('overview.projectUsage')}
        </button>
      </div>
      <div className="tt-tbl-scroll">
        <div className="tt-tbl-grid tt-tbl-head">
          {cols.map((c) => (
            <button
              key={c.key}
              className={sort.key === c.key ? 'active' : ''}
              onClick={() => clickCol(c.key)}
              title={c.key === 'reasoning' ? t('overview.reasoningNote') : undefined}
            >
              {t(c.labelKey)}
              <span className="arrow">{sort.key === c.key ? (sort.dir === 'asc' ? '▲' : '▼') : ''}</span>
            </button>
          ))}
        </div>
        {sorted.map((r, i) => (
          <div className="tt-tbl-grid tt-tbl-row" key={r.label + i}>
            <span>{tab === 'daily' ? fmtIsoDateL(r.label, lang) : r.label}</span>
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
