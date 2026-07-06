import { KeyRound } from 'lucide-react';

import { useDaemonFetch } from '@/lib/use-fetch';
import { type Entry, asString, entryList } from '@/lib/daemon';
import { formatRelativeTime } from '@/lib/format';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { ScrollArea } from '@/components/ui/scroll-area';
import { ViewLoading } from '@/components/view-states';

// Read-only access overview (DASHBOARD_UX §5.7). Aggregates grant.list /
// share.list / invite.list (all spine reads). Writes (grant.set / invite.create
// / share.create) are periphery → this view is intentionally view-only.
export function AccessView({ refreshNonce }: { refreshNonce: number }) {
  const grants = useDaemonFetch('grant.list', {}, refreshNonce);
  const shares = useDaemonFetch('share.list', {}, refreshNonce);
  const invites = useDaemonFetch('invite.list', {}, refreshNonce);

  const grantEntries = entryList(grants.data, ['grants', 'entries', 'items']);
  const shareEntries = entryList(shares.data, ['shares', 'entries', 'items']);
  const inviteEntries = entryList(invites.data, ['invites', 'entries', 'items']);

  if (grants.loading && shares.loading && invites.loading) return <ViewLoading rows={6} />;

  return (
    <ScrollArea className="h-full">
      <div className="mx-auto flex max-w-2xl flex-col gap-4 p-4">
        <AccessSection
          title="Grants"
          entries={grantEntries}
          loading={grants.loading}
          empty="No grants."
        />
        <AccessSection
          title="Shares"
          entries={shareEntries}
          loading={shares.loading}
          empty="No shared links."
        />
        <AccessSection
          title="Invites"
          entries={inviteEntries}
          loading={invites.loading}
          empty="No pending invites."
        />
        <p className="text-muted-foreground text-xs">
          Granting, revoking, and creating invites/shares require daemon support (periphery writes
          return method_not_implemented). This view is read-only for now.
        </p>
      </div>
    </ScrollArea>
  );
}

function AccessSection({
  title,
  entries,
  loading,
  empty,
}: {
  title: string;
  entries: Entry[];
  loading: boolean;
  empty: string;
}) {
  return (
    <Card className="py-4">
      <CardHeader className="pb-0">
        <CardTitle className="text-sm">{title}</CardTitle>
      </CardHeader>
      <CardContent>
        {loading ? (
          <p className="text-muted-foreground text-xs">Loading…</p>
        ) : entries.length === 0 ? (
          <p className="text-muted-foreground text-xs">{empty}</p>
        ) : (
          <ul className="flex flex-col gap-1">
            {entries.map((entry, index) => (
              <li
                key={`${title}:${String(index)}:${asString(entry.id ?? entry.name)}`}
                className="flex items-center gap-2 text-sm"
              >
                <KeyRound className="text-muted-foreground size-3.5" />
                <span className="min-w-0 flex-1 truncate">
                  {asString(entry.name ?? entry.subject ?? entry.id, '(unnamed)')}
                </span>
                {entry.role ? (
                  <span className="text-muted-foreground text-xs">{asString(entry.role)}</span>
                ) : null}
                {entry.expires ? (
                  <span className="text-muted-foreground text-xs">
                    {formatRelativeTime(entry.expires)}
                  </span>
                ) : null}
              </li>
            ))}
          </ul>
        )}
      </CardContent>
    </Card>
  );
}
