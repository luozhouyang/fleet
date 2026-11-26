import { useNavigate, useParams, useSearch } from '@tanstack/react-router';
import { Menu, MessageSquare } from 'lucide-react';
import { useEffect, useState } from 'react';
import { NewSessionDialog } from '@/components/dialogs/new-session-dialog';
import { SessionContextPanel } from '@/components/session/session-context-panel';
import { SessionConversation } from '@/components/session/session-conversation';
import { Button } from '@/components/ui/button';
import { EmptyState } from '@/components/ui/feedback';
import { useMachines } from '@/lib/query/hooks';
import { cn } from '@/lib/utils/cn';
import { SessionRailPanel } from './rail/session-rail-panel';

export function SessionsPage() {
  const params = useParams({ strict: false });
  const search = useSearch({ strict: false }) as { q?: string };
  const sessionId = typeof params.sessionId === 'string' ? params.sessionId : '';
  const navigate = useNavigate();
  const machinesQuery = useMachines();

  const [newSessionOpen, setNewSessionOpen] = useState(false);
  const [mobileSidebarOpen, setMobileSidebarOpen] = useState(false);
  const [panelOpen, setPanelOpen] = useState(
    () => typeof window === 'undefined' || window.matchMedia('(min-width: 1280px)').matches,
  );

  useEffect(() => {
    setMobileSidebarOpen(false);
  }, [sessionId]);

  return (
    <div className="relative flex h-full min-h-0 overflow-hidden bg-background">
      {mobileSidebarOpen ? (
        <button
          type="button"
          aria-label="Close session list"
          className="fixed inset-x-0 bottom-0 top-14 z-40 bg-foreground/25 md:hidden"
          onClick={() => setMobileSidebarOpen(false)}
        />
      ) : null}

      {panelOpen && sessionId ? (
        <button
          type="button"
          aria-label="Close context panel"
          className="fixed inset-0 top-14 z-30 bg-foreground/25 lg:hidden"
          onClick={() => setPanelOpen(false)}
        />
      ) : null}

      <aside
        aria-label="Sessions"
        role={mobileSidebarOpen ? 'dialog' : undefined}
        aria-modal={mobileSidebarOpen ? true : undefined}
        className={cn(
          'min-h-0 shrink-0 flex-col border-r border-border bg-panel',
          mobileSidebarOpen
            ? 'fixed bottom-0 left-0 top-14 z-50 flex w-[320px] max-w-[85vw] shadow-modal md:static md:bottom-auto md:top-auto md:z-auto md:w-[280px] md:max-w-none md:shadow-none xl:w-[320px]'
            : 'hidden md:flex md:w-[280px] xl:w-[320px]',
        )}
      >
        <SessionRailPanel
          selectedId={sessionId}
          onNewSession={() => setNewSessionOpen(true)}
          onCloseMobile={() => setMobileSidebarOpen(false)}
        />
      </aside>

      <div className="flex min-h-0 min-w-0 flex-1">
        <div className="flex min-h-0 min-w-0 flex-1 flex-col">
          {sessionId ? (
            <SessionConversation
              sessionId={sessionId}
              panelOpen={panelOpen}
              onTogglePanel={() => setPanelOpen((value) => !value)}
              onOpenSidebar={() => setMobileSidebarOpen(true)}
            />
          ) : (
            <div className="relative flex min-h-0 flex-1 items-center justify-center">
              <Button
                variant="secondary"
                size="sm"
                className="absolute left-4 top-4 md:hidden"
                onClick={() => setMobileSidebarOpen(true)}
              >
                <Menu className="size-3.5" aria-hidden="true" />
                Sessions
              </Button>
              <EmptyState
                icon={<MessageSquare className="size-4" />}
                title={search.q ? 'No session selected' : 'Select a session'}
                description="Choose a session from the sidebar or start a new one to begin."
                action={
                  <Button variant="primary" onClick={() => setNewSessionOpen(true)}>
                    New session
                  </Button>
                }
              />
            </div>
          )}
        </div>

        {sessionId ? (
          <SessionContextPanel
            sessionId={sessionId}
            open={panelOpen}
            onToggle={() => setPanelOpen((value) => !value)}
          />
        ) : null}
      </div>

      <NewSessionDialog
        open={newSessionOpen}
        onOpenChange={setNewSessionOpen}
        machines={machinesQuery.data ?? []}
        onCreated={(session) => {
          void navigate({
            to: '/sessions/$sessionId',
            params: { sessionId: session.id },
          });
        }}
      />
    </div>
  );
}
