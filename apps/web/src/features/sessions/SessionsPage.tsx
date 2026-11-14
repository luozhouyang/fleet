import { useNavigate, useParams } from '@tanstack/react-router';
import { Menu, MessageSquare } from 'lucide-react';
import { useEffect, useState } from 'react';
import { NewSessionDialog } from '@/components/dialogs/new-session-dialog';
import { SessionContextPanel } from '@/components/session/session-context-panel';
import { SessionConversation } from '@/components/session/session-conversation';
import { SessionSidebar } from '@/components/session/session-sidebar';
import { Button } from '@/components/ui/button';
import { EmptyState } from '@/components/ui/feedback';
import { useMachines } from '@/lib/query/hooks';

export function SessionsPage() {
  const params = useParams({ strict: false });
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

      <SessionSidebar
        selectedId={sessionId}
        mobileOpen={mobileSidebarOpen}
        onCloseMobile={() => setMobileSidebarOpen(false)}
        onNewSession={() => setNewSessionOpen(true)}
      />

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
                title="Select a session"
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
