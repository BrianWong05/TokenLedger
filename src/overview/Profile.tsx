import { memo } from 'react';
import type { ProfileView } from './data';
import { fmtTok } from '../lib/format';
import { useOverviewT } from './localize';

// The Profile: a portrait of the whole Ledger that no selection can change —
// fixed trailing windows on top, the Models with the largest lifetime share
// below, and the Ledger's span in the footer. It reports no Cost, so none of
// the Partial Cost / Display Currency rules reach it.
//
// Shares are of ALL lifetime tokens including Unattributed Usage, so they sum
// to less than 100% — the row bar's width is that same absolute share, which
// keeps bar and printed figure in agreement.
function Profile({ profile }: { profile: ProfileView }) {
  const { t } = useOverviewT();
  const tiles: { key: string; value: string }[] = [
    { key: 'overview.profile.d7', value: fmtTok(profile.d7) },
    { key: 'overview.profile.d30', value: fmtTok(profile.d30) },
    {
      key: 'overview.profile.perActiveDay',
      // No active day means the average is unknown, not zero — same rule that
      // keeps an Unpriced Model off $0.
      value: profile.perActiveDay === null ? '—' : fmtTok(profile.perActiveDay),
    },
    {
      key: 'overview.profile.sessions',
      value: profile.sessions30d === null ? '—' : fmtTok(profile.sessions30d),
    },
  ];

  return (
    <div className="tt-card tt-profile">
      <div className="tt-profile-tiles">
        {tiles.map((tile) => (
          <div className="tt-profile-tile" key={tile.key}>
            <div className="num">{tile.value}</div>
            <div className="lbl">{t(tile.key as Parameters<typeof t>[0])}</div>
          </div>
        ))}
      </div>

      <div className="tt-profile-models">
        {profile.models.length === 0 && <div className="tt-profile-empty">{t('overview.profile.empty')}</div>}
        {profile.models.map((m, i) => {
          // One value for the bar and the figure, so the two can never disagree
          // (and the width never carries a float artifact like 27.2000000003%).
          const pct = (m.share * 100).toFixed(1) + '%';
          return (
            <div className="tt-profile-model" key={m.name}>
              <div className="fill" style={{ width: pct }} />
              <div className="row">
                <span className="rank">{i + 1}</span>
                <span className="name">{m.name}</span>
                <span className="tok">{fmtTok(m.tokens)}</span>
                <span className="pct" title={t('overview.profile.shareTitle')}>{pct}</span>
              </div>
            </div>
          );
        })}
      </div>

      <div className="tt-profile-foot">
        <span>
          {t('overview.profile.started')} <b>{profile.startedIso ?? '—'}</b>
        </span>
        <span>
          {t('overview.profile.activeDays')} <b>{profile.activeDays}</b>
        </span>
      </div>
    </div>
  );
}

// Memoized: the hook hands back an identity-stable view across the shell's
// per-tick re-renders (it only rebuilds when the series or Session count moves).
export default memo(Profile);
