import { memo, useState } from 'react';
import type { ProfileView } from './data';
import { fill, fmtTok } from '../lib/format';
import { useOverviewT } from './localize';

// How many Models the card shows before the reader asks for the rest.
const TOP_N = 5;

// The Profile: the Models of the selected window, ranked, across every Source —
// the top five, the rest one click away — with the window's own span in the
// footer (first active day + active-day count, both scoped to the range, TOKL-7).
// It reports no Cost, so none of the Partial Cost / Display Currency rules reach
// it. No heading above the rows on purpose: the range picker already names the
// window (TOKL-6).
//
// Shares are of the window's whole total including Unattributed Usage, so they
// sum to less than 100% — the row bar's width is that same absolute share,
// which keeps bar and printed figure in agreement.
function Profile({ profile }: { profile: ProfileView }) {
  const { t } = useOverviewT();
  const [expanded, setExpanded] = useState(false);
  const models = expanded ? profile.models : profile.models.slice(0, TOP_N);
  const hasMore = profile.models.length > TOP_N;

  return (
    <div className="tt-card tt-profile">
      <div className="tt-profile-models">
        {/* No Models is two different windows: an empty one, and one whose
            usage is all Unattributed. Saying "no usage" for the second would
            deny tokens the headline is counting. */}
        {profile.models.length === 0 && (
          <div className="tt-profile-empty">
            {t(profile.windowTokens > 0 ? 'overview.profile.unattributedOnly' : 'overview.profile.empty')}
          </div>
        )}
        {models.map((m, i) => {
          // One value for the bar and the figure, so the two can never disagree
          // (and the width never carries a float artifact like 27.2000000003%).
          const pct = (m.share * 100).toFixed(1) + '%';
          return (
            <div className="tt-profile-model" key={m.name}>
              <div className="fill" style={{ width: pct }} />
              {/* the spaces are load-bearing: flex drops whitespace-only items, so they
                  cost no layout, but without them the row's text content runs together
                  ("1claude-fable-516.51B53.6%") for screen readers and copy-paste */}
              <div className="row">
                <span className="rank">{i + 1}</span>{' '}
                <span className="name">{m.name}</span>{' '}
                <span className="tok">{fmtTok(m.tokens)}</span>{' '}
                <span className="pct" title={t('overview.profile.shareTitle')}>{pct}</span>
              </div>
            </div>
          );
        })}
      </div>

      {hasMore && (
        <button
          type="button"
          className="tt-profile-more"
          aria-expanded={expanded}
          onClick={() => setExpanded((x) => !x)}
        >
          {expanded
            ? fill(t('overview.profile.showTop'), { n: TOP_N })
            : fill(t('overview.profile.showAll'), { n: profile.models.length })}
        </button>
      )}

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
// per-tick re-renders (it only rebuilds when the window's Models or the series
// move).
export default memo(Profile);
