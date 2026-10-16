import { useState } from 'react';
import { Pencil, Plus, Trash2 } from 'lucide-react';
import { api } from './api';
import type { Schedule } from './model';
import { Modal } from './ui';

const dateText = (value: unknown) => new Date(String(value)).toLocaleString('zh-CN', { month: 'long', day: 'numeric', hour: '2-digit', minute: '2-digit' });
export const scheduleRuleText = (rule: Record<string, unknown>) => {
  if (rule.type === 'once') return `一次性 · ${dateText(rule.at)}`;
  if (rule.type === 'interval') return `每 ${rule.seconds} 秒`;
  if (rule.type === 'weekly') return `每周 ${(rule.weekdays as number[]).map(index => '一二三四五六日'[index]).join('、')} · ${rule.time}`;
  if (rule.type === 'monthly') return `每月 ${rule.day} 日 · ${rule.time}`;
  return `每天 · ${rule.time}`;
};

/** Edits the same Host plans projected by Timeline; it does not own a second schedule store. */
export function ScheduleManager({ graphs, schedules, refresh, close }: { graphs: string[]; schedules: Schedule[]; refresh: () => void; close: () => void }) {
  const [editing, setEditing] = useState<string | null>(null);
  const [graph, setGraph] = useState(graphs[0] ?? '');
  const [rule, setRule] = useState('once');
  const [at, setAt] = useState('');
  const [time, setTime] = useState('09:00');
  const [interval, setInterval] = useState(3600);
  const [input, setInput] = useState('');
  const [weekdays, setWeekdays] = useState([0, 1, 2, 3, 4]);
  const [monthDay, setMonthDay] = useState(1);
  const [enabled, setEnabled] = useState(true);
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');
  const [pending, setPending] = useState(false);
  const [deleting, setDeleting] = useState<string | null>(null);
  const selectedGraph = graph || (!editing ? graphs[0] ?? '' : '');
  const edit = (item: Schedule | null) => {
    setEditing(item?.id ?? null); setGraph(item?.graph ?? graphs[0] ?? '');
    setRule(String(item?.rule.type ?? 'once')); setAt(String(item?.rule.at ?? ''));
    setTime(String(item?.rule.time ?? '09:00')); setInterval(Number(item?.rule.seconds ?? 3600));
    setWeekdays((item?.rule.weekdays as number[]) ?? [0, 1, 2, 3, 4]); setMonthDay(Number(item?.rule.day ?? 1));
    setInput(item ? JSON.stringify(item.input ?? {}, null, 2) : ''); setEnabled(item?.enabled ?? true);
    setError(''); setNotice(''); setDeleting(null);
  };
  const mutate = async (action: () => Promise<unknown>, message: string) => {
    if (pending) return;
    setPending(true); setError(''); setNotice('');
    try { await action(); refresh(); setNotice(message); }
    catch (cause) { setError((cause as Error).message); }
    finally { setPending(false); }
  };
  const save = () => mutate(async () => {
    const value: unknown = input.trim() ? JSON.parse(input) : {};
    if (!value || Array.isArray(value) || typeof value !== 'object') throw new Error('计划输入需要一个 JSON 对象。');
    const scheduleRule = rule === 'once' ? { type: rule, at } : rule === 'interval' ? { type: rule, seconds: interval }
      : rule === 'daily' ? { type: rule, time } : rule === 'weekly' ? { type: rule, time, weekdays } : { type: rule, time, day: monthDay };
    const result = await api<{ schedule: Schedule }>(editing ? `/schedules/${encodeURIComponent(editing)}` : '/schedules', editing ? 'PUT' : 'POST',
      { graph: selectedGraph, rule: scheduleRule, input: value, ...(editing ? { enabled } : {}) });
    setEditing(result.schedule.id);
  }, editing ? '计划已更新，已启动的运行保持原样。' : '计划已创建。');
  return <Modal title="定时计划" close={() => { if (!pending) close(); }} className="timeline-schedule-dialog">
    <p className="timeline-muted">按服务所在时区执行。停机或工作流忙碌时跳过，不补跑。</p>
    <form onSubmit={event => { event.preventDefault(); void save(); }}>
      <fieldset disabled={pending} className="schedule-fields">
        <div className="schedule-form-heading"><h3>{editing ? '编辑计划' : '新建计划'}</h3>{editing && <button type="button" onClick={() => edit(null)}><Plus size={14} />新建另一计划</button>}</div>
        <label>工作流<select required value={selectedGraph} onChange={event => setGraph(event.target.value)}>
          {!graphs.includes(selectedGraph) && selectedGraph && <option value={selectedGraph}>{selectedGraph}（当前不可用）</option>}
          {graphs.map(name => <option key={name}>{name}</option>)}
        </select></label>
        <label>规则<select value={rule} onChange={event => setRule(event.target.value)}><option value="once">未来执行一次</option><option value="interval">固定间隔</option><option value="daily">每天</option><option value="weekly">每周</option><option value="monthly">每月</option></select></label>
        {rule === 'once' ? <label>执行时间<input required type="datetime-local" value={at} onChange={event => setAt(event.target.value)} /></label> : rule === 'interval' ? <label>间隔秒数<input required type="number" min="1" step="1" value={interval} onChange={event => setInterval(Number(event.target.value))} /></label> : <label>服务所在时区的时间<input required type="time" value={time} onChange={event => setTime(event.target.value)} /></label>}
        {rule === 'weekly' && <fieldset className="weekdays"><legend>星期</legend>{['一', '二', '三', '四', '五', '六', '日'].map((label, index) => <label key={index}><input type="checkbox" checked={weekdays.includes(index)} onChange={event => setWeekdays(old => event.target.checked ? [...old, index].sort() : old.filter(value => value !== index))} />{label}</label>)}</fieldset>}
        {rule === 'monthly' && <label>每月日期<input required type="number" min="1" max="31" value={monthDay} onChange={event => setMonthDay(Number(event.target.value))} /></label>}
        <label>运行输入（可选 JSON）<textarea rows={3} value={input} onChange={event => setInput(event.target.value)} placeholder="{}" /></label>
        {editing && <label className="schedule-enabled"><input type="checkbox" checked={enabled} onChange={event => setEnabled(event.target.checked)} />启用此计划</label>}
        <button className="primary" disabled={!selectedGraph} type="submit">{pending ? '正在保存…' : editing ? '保存修改' : '保存计划'}</button>
      </fieldset>
    </form>
    {error && <p className="timeline-error" role="alert">{error}</p>}
    {notice && <p className="notice ok" role="status">{notice}</p>}
    <section className="schedule-list" aria-label="已配置的计划"><h3>已配置 · {schedules.length}</h3>
      {!schedules.length && <p className="timeline-muted">还没有计划。保存后即可在时间线上查看。</p>}
      {schedules.map(item => <div className="schedule-item" key={item.id}>
        <div><strong>{item.graph}</strong><span>{scheduleRuleText(item.rule)}</span><small>{item.enabled ? `下次 ${dateText(item.next_at)}` : item.rule.type === 'once' && new Date(String(item.rule.at)).valueOf() < Date.now() ? '已结束' : '已暂停'}</small></div>
        <div className="schedule-item-actions">
          <button disabled={pending} aria-label={`编辑 ${item.graph} 计划`} onClick={() => edit(item)}><Pencil size={14} /></button>
          <button disabled={pending} onClick={() => void mutate(async () => { const result = await api<{ schedule: Schedule }>(`/schedules/${encodeURIComponent(item.id)}`, 'PUT', { graph: item.graph, rule: item.rule, input: item.input ?? {}, enabled: !item.enabled }); if (editing === item.id) setEnabled(result.schedule.enabled); }, item.enabled ? '计划已暂停。已启动的运行会继续。' : '计划已启用，将从未来时点开始执行。')}>{item.enabled ? '暂停' : '启用'}</button>
          <button disabled={pending} aria-label={`删除 ${item.graph} 计划`} onClick={() => setDeleting(item.id)}><Trash2 size={14} /></button>
          {deleting === item.id && <div className="schedule-delete-confirm"><span>删除计划？过去的运行会保留。</span><button disabled={pending} onClick={() => setDeleting(null)}>取消</button><button className="danger-link" disabled={pending} onClick={() => void mutate(async () => { await api(`/schedules/${encodeURIComponent(item.id)}`, 'DELETE'); if (editing === item.id) edit(null); setDeleting(null); }, '计划已删除，过去的运行已保留。')}>确认删除</button></div>}
        </div>
      </div>)}
    </section>
  </Modal>;
}
