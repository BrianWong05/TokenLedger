import type { BreakdownRow } from '../types';
import { formatTokens, formatCost } from '../lib/format';

export interface BreakdownTablesProps {
  modelRows: BreakdownRow[];
  projectRows: BreakdownRow[];
}

function basename(path: string): string {
  if (path === 'unknown') return 'unknown';
  const parts = path.split('/').filter(Boolean);
  return parts.length ? parts[parts.length - 1] : path;
}

function Row({ row, label, title }: { row: BreakdownRow; label: string; title?: string }) {
  return (
    <tr>
      <td className="bt-key" title={title}>
        {label}
      </td>
      <td className="bt-num">{formatTokens(row.inputTokens)}</td>
      <td className="bt-num">{formatTokens(row.outputTokens)}</td>
      <td className="bt-num">{formatTokens(row.cacheReadTokens + row.cacheWriteTokens)}</td>
      <td className="bt-num">{formatTokens(row.requests)}</td>
      <td className="bt-num">{formatCost(row.cost, false)}</td>
    </tr>
  );
}

function Table({
  heading,
  keyLabel,
  rows,
  isProject,
}: {
  heading: string;
  keyLabel: string;
  rows: BreakdownRow[];
  isProject: boolean;
}) {
  return (
    <div className="breakdown-table">
      <h3 className="bt-heading">{heading}</h3>
      <table>
        <thead>
          <tr>
            <th>{keyLabel}</th>
            <th>Input</th>
            <th>Output</th>
            <th>Cache</th>
            <th>Requests</th>
            <th>Est. cost</th>
          </tr>
        </thead>
        <tbody>
          {rows.length === 0 ? (
            <tr>
              <td className="bt-empty" colSpan={6}>
                No data
              </td>
            </tr>
          ) : (
            rows.map((r) =>
              isProject ? (
                <Row key={r.key} row={r} label={basename(r.key)} title={r.key} />
              ) : (
                <Row key={r.key} row={r} label={r.key} />
              ),
            )
          )}
        </tbody>
      </table>
    </div>
  );
}

export default function BreakdownTables({ modelRows, projectRows }: BreakdownTablesProps) {
  return (
    <div className="breakdown-tables">
      <Table heading="By model" keyLabel="Model" rows={modelRows} isProject={false} />
      <Table heading="By project" keyLabel="Project" rows={projectRows} isProject={true} />
    </div>
  );
}
