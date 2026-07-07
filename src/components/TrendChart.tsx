import {
  ComposedChart,
  Area,
  Line,
  XAxis,
  YAxis,
  CartesianGrid,
  Tooltip,
  Legend,
  ResponsiveContainer,
} from 'recharts';
import type { TrendPoint } from '../types';
import { formatTokens } from '../lib/format';

export interface TrendChartProps {
  points: TrendPoint[];
  bucket: 'day' | 'hour';
}

// Mirrors the --chart-* CSS variables in index.css (Task 15). Hardcoded because
// WebKit (Tauri's macOS webview) does not resolve var() in SVG presentation attributes.
const CHART = {
  input: '#7c5cff',
  output: '#2fbf71',
  cacheRead: '#3aa0ff',
  cacheWrite: '#f0a03c',
  cost: '#ff5c7a',
} as const;

export default function TrendChart({ points, bucket }: TrendChartProps) {
  const tickFormat = (v: string) =>
    bucket === 'hour' ? v.split(' ')[1] ?? v : v.slice(5);

  if (points.length === 0) {
    return (
      <div className="trend-chart">
        <h3 className="trend-heading">Usage over time</h3>
        <div className="trend-empty">No data</div>
      </div>
    );
  }

  return (
    <div className="trend-chart">
      <h3 className="trend-heading">Usage over time</h3>
      <ResponsiveContainer width="100%" height={320}>
        <ComposedChart data={points} margin={{ top: 8, right: 16, bottom: 8, left: 8 }}>
          <CartesianGrid strokeDasharray="3 3" />
          <XAxis dataKey="bucket" tickFormatter={tickFormat} tick={{ fontSize: 12 }} />
          <YAxis
            yAxisId="tokens"
            tickFormatter={(v) => formatTokens(v as number)}
            tick={{ fontSize: 12 }}
          />
          <YAxis
            yAxisId="cost"
            orientation="right"
            tickFormatter={(v) => `$${(v as number).toFixed(0)}`}
            tick={{ fontSize: 12 }}
          />
          <Tooltip
            formatter={(value, name) =>
              name === 'Est. cost'
                ? `$${(value as number).toFixed(2)}`
                : formatTokens(value as number)
            }
          />
          <Legend />
          <Area
            yAxisId="tokens"
            type="monotone"
            dataKey="inputTokens"
            name="Input"
            stackId="tok"
            stroke={CHART.input}
            fill={CHART.input}
            fillOpacity={0.7}
          />
          <Area
            yAxisId="tokens"
            type="monotone"
            dataKey="outputTokens"
            name="Output"
            stackId="tok"
            stroke={CHART.output}
            fill={CHART.output}
            fillOpacity={0.7}
          />
          <Area
            yAxisId="tokens"
            type="monotone"
            dataKey="cacheReadTokens"
            name="Cache read"
            stackId="tok"
            stroke={CHART.cacheRead}
            fill={CHART.cacheRead}
            fillOpacity={0.7}
          />
          <Area
            yAxisId="tokens"
            type="monotone"
            dataKey="cacheWriteTokens"
            name="Cache write"
            stackId="tok"
            stroke={CHART.cacheWrite}
            fill={CHART.cacheWrite}
            fillOpacity={0.7}
          />
          <Line
            yAxisId="cost"
            type="monotone"
            dataKey="cost"
            name="Est. cost"
            stroke={CHART.cost}
            strokeWidth={2}
            dot={false}
          />
        </ComposedChart>
      </ResponsiveContainer>
    </div>
  );
}
