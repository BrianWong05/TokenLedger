import { useState } from 'react';
import type { OverrideRates } from '../types';

export interface PriceOverrideDialogProps {
  model: string;
  onSave: (model: string, rates: OverrideRates) => void;
  onDelete: (model: string) => void;
  onClose: () => void;
}

// $ per 1M tokens (display) -> per-token (stored). Blank / invalid -> null.
function toPerTok(perM: string): number | null {
  const v = parseFloat(perM);
  return Number.isFinite(v) ? v / 1_000_000 : null;
}

export default function PriceOverrideDialog({
  model,
  onSave,
  onDelete,
  onClose,
}: PriceOverrideDialogProps) {
  const [input, setInput] = useState('');
  const [output, setOutput] = useState('');
  const [cacheRead, setCacheRead] = useState('');
  const [cacheWrite, setCacheWrite] = useState('');

  const handleSave = () => {
    onSave(model, {
      input: toPerTok(input),
      output: toPerTok(output),
      cacheRead: toPerTok(cacheRead),
      cacheWrite: toPerTok(cacheWrite),
    });
  };

  return (
    <div className="dialog-backdrop" onClick={onClose}>
      <div
        className="dialog"
        role="dialog"
        aria-label={`Set price for ${model}`}
        onClick={(e) => e.stopPropagation()}
      >
        <div className="dialog-title">Set price for {model}</div>
        <div className="dialog-sub">$ / 1M tokens</div>

        <label className="dialog-field">
          <span>Input</span>
          <input
            type="number"
            min="0"
            step="any"
            value={input}
            onChange={(e) => setInput(e.target.value)}
          />
        </label>
        <label className="dialog-field">
          <span>Output</span>
          <input
            type="number"
            min="0"
            step="any"
            value={output}
            onChange={(e) => setOutput(e.target.value)}
          />
        </label>
        <label className="dialog-field">
          <span>Cache read</span>
          <input
            type="number"
            min="0"
            step="any"
            value={cacheRead}
            onChange={(e) => setCacheRead(e.target.value)}
          />
        </label>
        <label className="dialog-field">
          <span>Cache write</span>
          <input
            type="number"
            min="0"
            step="any"
            value={cacheWrite}
            onChange={(e) => setCacheWrite(e.target.value)}
          />
        </label>

        <div className="dialog-actions">
          <button
            type="button"
            className="dialog-delete"
            onClick={() => onDelete(model)}
          >
            Delete override
          </button>
          <button type="button" className="dialog-cancel" onClick={onClose}>
            Cancel
          </button>
          <button type="button" className="dialog-save" onClick={handleSave}>
            Save
          </button>
        </div>
      </div>
    </div>
  );
}
