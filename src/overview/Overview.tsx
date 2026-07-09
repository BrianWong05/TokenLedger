import { useState } from 'react';
import './overview.css';
import Heatmap from './Heatmap';
import TrendBars from './TrendBars';
import FocusPanel from './FocusPanel';

const NAV = ['Overview', 'Activity', 'Models', 'Settings'];

// Design 8a — "App · Overview". Static app shell (nav is decorative) wrapping
// the three live panels. All figures come from ./mock (fake data).
export default function Overview() {
  const [nav, setNav] = useState('Overview');

  return (
    <div className="tt">
      <div className="tt-app">
        <div className="tt-top">
          <div className="tt-brand">
            <div className="tt-logo">
              <i>T</i>
              <b>tokentracker</b>
            </div>
            <div className="tt-nav">
              {NAV.map((n) => (
                <button key={n} className={n === nav ? 'active' : ''} onClick={() => setNav(n)}>
                  {n}
                </button>
              ))}
            </div>
          </div>
          <div className="tt-top-right">
            <span className="tt-year">
              2025 <span>▼</span>
            </span>
            <span className="tt-avatar">MK</span>
          </div>
        </div>

        <div className="tt-grid">
          <div className="tt-left">
            <Heatmap />
            <TrendBars />
          </div>
          <FocusPanel />
        </div>
      </div>
    </div>
  );
}
