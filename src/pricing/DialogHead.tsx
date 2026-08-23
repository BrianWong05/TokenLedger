import type { RefObject } from 'react';
import { sourceIcon } from '../overview/icons';
import { pricingSourceMeta, sourceLabel } from './pricing.derive';

// The tl-pr dialog family's shared header: Source icon chip, Model name (the
// dialog's aria label), subtitle, close. One component so the Override editor
// and the Model breakdown can't drift apart. The two dialogs are never mounted
// at once (the breakdown closes as it hands off), so the shared name id stays
// unique in the document.
export default function DialogHead({
  tool,
  name,
  subtitle,
  closeLabel,
  onClose,
  closeRef,
}: {
  tool: string;
  name: string;
  subtitle: string;
  closeLabel: string;
  onClose: () => void;
  closeRef?: RefObject<HTMLButtonElement>;
}) {
  const icon = sourceIcon(pricingSourceMeta(tool).icon);
  const label = sourceLabel(tool);
  return (
    /* the backdrop covers the shell's drag handles, so the dialog's own
       header is one ("deep": the whole strip drags, buttons still click) */
    <div className="tl-pr-dialog-head" data-tauri-drag-region="deep">
      <span className={'tl-pr-icon ' + tool}>
        {icon ? <img src={icon} alt="" width={15} height={15} /> : <b>{label[0]}</b>}
      </span>
      <div style={{ flex: 1, minWidth: 0 }}>
        <div className="name" id="tl-pr-dialog-name">{name}</div>
        <div className="subtitle">{subtitle}</div>
      </div>
      <button ref={closeRef} type="button" className="tl-pr-dialog-close" aria-label={closeLabel} onClick={onClose}>
        ✕
      </button>
    </div>
  );
}
