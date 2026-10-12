import { useEffect, useState } from 'react';
import { PilotQuestion } from './PilotQuestion';
import { loadQuestionHistory, type Question } from './pilotQuestions';

export function PilotQuestionHistory({ session, turns }: { session: string; turns: string[] }) {
  const [history, setHistory] = useState<{ session: string; identifiers: string; questions: Question[]; problem: string } | null>(null);
  const [reload, setReload] = useState(0);
  const identifiers = JSON.stringify(turns);
  useEffect(() => {
    const controller = new AbortController();
    setHistory(null);
    void loadQuestionHistory(session, JSON.parse(identifiers) as string[], controller.signal).then(result => {
      if (!controller.signal.aborted) setHistory({ session, identifiers, questions: result, problem: '' });
    }).catch(error => {
      if (!controller.signal.aborted) setHistory({ session, identifiers, questions: [], problem: (error as Error).message });
    });
    return () => controller.abort();
  }, [session, identifiers, reload]);
  if (!history || history.session !== session || history.identifiers !== identifiers) return null;
  if (history.problem) return <p className="pilot-error" role="alert">无法加载历史提问：{history.problem}
    <button type="button" onClick={() => setReload(previous => previous + 1)}>重新加载历史提问</button>
  </p>;
  if (!history.questions.length) return null;
  return <details className="pilot-tools pilot-question-history">
    <summary>历史提问 · {history.questions.length} 项</summary>
    <div className="pilot-question-history-list">
      {history.questions.map(question => <PilotQuestion key={`${question.turn}-${question.id}`} question={question} enabled={false} onAnswered={() => undefined} />)}
    </div>
  </details>;
}
