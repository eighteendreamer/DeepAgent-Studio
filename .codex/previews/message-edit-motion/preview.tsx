import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { createRoot } from 'react-dom/client';
import gsap from 'gsap';
import { ArrowUp, Check, ChevronRight, Clock3, Copy, Feather, Brain, Lightbulb, Moon, Play, Plus, Sun } from 'lucide-react';
import { Button } from '../../../apps/desktop/src/components/shadcn/button';
import { TintButton } from '../../../apps/desktop/src/components/ui/TintButton';
import { InputSurface } from '../../../apps/desktop/src/components/ui/InputSurface';
import { ModelThinkingSelector } from '../../../apps/desktop/src/components/ModelThinkingSelector';
import { SHARE_MOTION, prefersReducedMotion, springEase } from '../../../apps/desktop/src/components/ui/springMotion';
import { continuousRadius, CORNER_CURVATURE } from '../../../apps/desktop/src/theme/continuousCorners';
import { writeClipboardText } from '../../../apps/desktop/src/clipboard';

type PlanId = 'A' | 'B' | 'C' | 'D' | 'E' | 'F';
type Settings = { width: number; speed: number; delay: number; blur: number; direction: 'down' | 'up' };
const PLANS: { id: PlanId; name: string; description: string; tag: string; settings: Settings }[] = [
  { id: 'A', name: '同源舒展', description: '右侧对齐，气泡变成编辑框', tag: '最接近模型菜单', settings: { width: 82, speed: 1, delay: 57, blur: 5, direction: 'down' } },
  { id: 'B', name: '贴底上展', description: '向左上展开，回答留在原处', tag: '下沿锚定', settings: { width: 82, speed: 1, delay: 57, blur: 5, direction: 'up' } },
  { id: 'C', name: '分步铺开', description: '先拉开宽度，再展开编辑区', tag: '分段节奏', settings: { width: 82, speed: .9, delay: 220, blur: 4, direction: 'down' } },
  { id: 'D', name: '整行编辑', description: '铺至对话宽度，适合长文本', tag: '宽幅工作区', settings: { width: 100, speed: .92, delay: 32, blur: 3, direction: 'down' } },
  { id: 'E', name: '紧凑轻弹', description: '较小的编辑框，更快地收回', tag: '短消息优先', settings: { width: 65, speed: .68, delay: 20, blur: 2, direction: 'down' } },
  { id: 'F', name: '自己调手感', description: '调整展开方向、宽度和节奏', tag: 'DIY', settings: { width: 82, speed: 1, delay: 57, blur: 5, direction: 'down' } },
];
const SHORT = '你是谁';
const LONG = '帮我检查一下消息气泡的编辑体验。\n我希望它像模型菜单一样，从原来的气泡自然展开，取消时再收回。\n保留苹果连续圆角，也要留意下面 AI 回答的位置。';
const ANSWER = '雷师傅，我是 DeepAgent，一个运行在你项目里的 Rust 原生编码智能体。能读代码、改文件、跑命令、查资料，帮你完成软件工程任务。';
const THINKING = [{ id: 'simple' as const, label: '简单', icon: Feather }, { id: 'medium' as const, label: '中度', icon: Lightbulb }, { id: 'deep' as const, label: '深度', icon: Brain }];

function EditStage({ plan, settings, seed, slow, autoPlay, notify }: {
  plan: PlanId; settings: Settings; seed: string; slow: boolean; autoPlay: boolean; notify: (text: string) => void;
}) {
  const [message, setMessage] = useState(seed), [draft, setDraft] = useState(seed), [open, setOpen] = useState(false);
  const [modelOpen, setModelOpen] = useState(false), [model, setModel] = useState('deepseek-flash');
  const [thinking, setThinking] = useState<'simple' | 'medium' | 'deep'>('medium');
  const rowRef = useRef<HTMLDivElement>(null), shellRef = useRef<HTMLDivElement>(null);
  const probeRef = useRef<HTMLDivElement>(null), compactRef = useRef<HTMLDivElement>(null);
  const editorRef = useRef<HTMLDivElement>(null), inputRef = useRef<HTMLTextAreaElement>(null);
  const actionsRef = useRef<HTMLDivElement>(null), editButton = useRef<HTMLButtonElement>(null);
  const previousOpen = useRef(false), initialized = useRef(false);
  const resizeRef = useRef<() => void>();
  const edit = () => { setDraft(message); setOpen(true); };
  const cancel = () => { setOpen(false); setDraft(message); };
  const apply = () => { if (!draft.trim()) return; setMessage(draft.trim()); setOpen(false); notify('已更新本页的示例消息'); };

  useEffect(() => { if (autoPlay) edit(); }, []);
  useLayoutEffect(() => {
    const row = rowRef.current!, shell = shellRef.current!, probe = probeRef.current!;
    const compact = compactRef.current!, editor = editorRef.current!, input = inputRef.current!, actions = actionsRef.current!;
    const first = !initialized.current, entering = open && !previousOpen.current, leaving = !open && previousOpen.current;
    initialized.current = true; previousOpen.current = open;
    const reduced = prefersReducedMotion();
    const multiplier = settings.speed * (slow ? 1.8 : 1);
    let timeline: gsap.core.Timeline | undefined, resizing: gsap.core.Timeline | undefined;
    let disposed = false, targetWidth = 0, targetHeight = 0, lastWidth = row.clientWidth;
    const dimensions = () => {
      const closed = { width: probe.offsetWidth, height: probe.offsetHeight };
      const width = Math.min(row.clientWidth, Math.max(Math.min(320, row.clientWidth), row.clientWidth * settings.width / 100));
      editor.style.width = `${width}px`;
      input.style.height = 'auto';
      input.style.height = `${Math.min(144, Math.max(48, input.scrollHeight))}px`;
      const height = editor.offsetHeight;
      const scroller = row.closest('.conversation-scroll')!;
      const availableAbove = Math.max(0, row.getBoundingClientRect().top - scroller.getBoundingClientRect().top - 10);
      const y = settings.direction === 'up' ? Math.max(closed.height - height, -availableAbove) : 0;
      const flow = Math.max(closed.height + 28, y + height + 8);
      return { closed, width, height, y, flow };
    };
    let size = dimensions(); targetWidth = size.width; targetHeight = size.height;
    compact.style.width = `${size.closed.width}px`;
    compact.inert = open; editor.inert = !open;
    if (first) {
      gsap.set(shell, { width: size.closed.width, height: size.closed.height, y: 0,
        '--continuous-corner-curvature': CORNER_CURVATURE,
        borderRadius: continuousRadius(16), borderTopRightRadius: continuousRadius(4), boxShadow: '0 0 0 rgba(0,0,0,0)' });
      gsap.set(row, { height: size.closed.height + 28 });
      gsap.set(editor, { opacity: 0, filter: `blur(${settings.blur}px)` });
      gsap.set(compact, { opacity: 1 });
    }
    const finish = () => {
      if (disposed) return;
      shell.dataset.motionRunning = 'false';
      if (!open && leaving) editButton.current?.focus({ preventScroll: true });
    };
    if (first && !open) finish();
    else {
      shell.dataset.motionRunning = 'true';
      const m = open ? SHARE_MOTION.open : SHARE_MOTION.close;
      const widthDuration = (plan === 'C' && open ? .42 : m.width.duration) * multiplier;
      const heightDuration = m.height.duration * multiplier;
      const heightDelay = (open ? settings.delay / 1000 : m.height.delay) * multiplier;
      const widthEase = springEase(m.width.stiffness, m.width.damping, plan === 'C' && open ? .42 : m.width.duration);
      const heightEase = springEase(m.height.stiffness, m.height.damping, m.height.duration);
      timeline = gsap.timeline({ onComplete: finish });
      const d = (value: number) => reduced ? 0 : value;
      timeline.to(shell, { width: open ? size.width : size.closed.width, duration: d(widthDuration), ease: widthEase }, d(m.width.delay * multiplier));
      timeline.to(shell, { height: open ? size.height : size.closed.height, y: open ? size.y : 0,
        duration: d(heightDuration), ease: heightEase }, d(heightDelay));
      // A single slot follows the same height tween. There is no second action-row exit.
      timeline.to(row, { height: open ? size.flow : size.closed.height + 28,
        duration: d(heightDuration), ease: heightEase }, d(heightDelay));
      timeline.to(shell, { borderRadius: continuousRadius(open ? 20 : 16), borderTopRightRadius: continuousRadius(open ? 20 : 4),
        boxShadow: open ? '0 6px 24px rgba(0,0,0,.08)' : '0 0 0 rgba(0,0,0,0)', duration: d(.22 * multiplier), ease: 'power2.out' }, 0);
      timeline.to(compact, { opacity: open ? 0 : 1, duration: d(.14 * multiplier), ease: 'power2.out' }, d(open ? 0 : .2 * multiplier));
      timeline.to(actions, { opacity: open ? 0 : 1, duration: d(.14 * multiplier), ease: 'power2.out' }, d(open ? 0 : .2 * multiplier));
      timeline.to(editor, { opacity: open ? 1 : 0, filter: `blur(${open ? 0 : settings.blur}px)`, duration: d((open ? .24 : .18) * multiplier), ease: 'power2.out' }, d(open ? .12 * multiplier : 0));
      if (entering) {
        timeline.fromTo(editor.querySelectorAll('[data-editor-action]'), { opacity: 0, y: 6 },
          { opacity: 1, y: 0, duration: d(.5 * multiplier), stagger: reduced ? 0 : .025 * multiplier, ease: springEase(280, 24, .5) }, d(.13 * multiplier));
        timeline.call(() => {
          if (disposed) return;
          input.focus({ preventScroll: true }); input.setSelectionRange(input.value.length, input.value.length);
        }, [], d(.36 * multiplier));
      }
    }
    const retarget = () => {
      if (disposed) return;
      const next = dimensions();
      if (open && Math.abs(next.width-targetWidth)<.5 && Math.abs(next.height-targetHeight)<.5) return;
      const widthChanged = Math.abs(next.width-targetWidth)>=.5;
      targetWidth = next.width; targetHeight = next.height; size = next;
      resizing?.kill();
      if (!open) {
        compact.style.width = `${next.closed.width}px`;
        gsap.set(shell, { width: next.closed.width, height: next.closed.height, y: 0 });
        gsap.set(row, { height: next.closed.height + 28 }); return;
      }
      gsap.killTweensOf(shell, widthChanged ? 'width,height,y' : 'height,y');
      gsap.killTweensOf(row, 'height');
      const m = SHARE_MOTION.resize;
      resizing = gsap.timeline();
      resizing.to(shell, { ...(widthChanged ? { width: next.width } : {}), height: next.height, y: next.y,
        duration: reduced ? 0 : m.duration * multiplier, ease: springEase(m.stiffness, m.damping, m.duration) }, 0);
      resizing.to(row, { height: next.flow, duration: reduced ? 0 : m.duration * multiplier, ease: springEase(m.stiffness, m.damping, m.duration) }, 0);
    };
    resizeRef.current = retarget;
    const observer = new ResizeObserver(() => { if (row.clientWidth !== lastWidth) { lastWidth = row.clientWidth; retarget(); } });
    observer.observe(row);
    return () => { disposed = true; timeline?.kill(); resizing?.kill(); observer.disconnect(); resizeRef.current = undefined; };
  }, [open, settings, message, slow]);
  useLayoutEffect(() => { if (open) resizeRef.current?.(); }, [draft]);

  return <section className="demo-stage" aria-label={`方案 ${plan} 的对话预览`}>
    <div className="demo-titlebar"><span>DeepAgent 身份介绍</span><span className="demo-status"><i />示例对话</span></div>
    <div className="conversation-scroll">
      <div className="conversation-content">
        <div ref={rowRef} className="demo-user-row" data-user-row>
          <div ref={probeRef} aria-hidden="true" className="bubble-probe">{message}</div>
          <div ref={shellRef} className="demo-bubble-shell" data-editor-shell data-state={open ? 'open' : 'closed'} data-motion-running="false">
            <div ref={compactRef} className="bubble-compact" aria-hidden={open}>{message}</div>
            <div ref={editorRef} className="bubble-editor" aria-hidden={!open}>
              <textarea ref={inputRef} aria-label="编辑示例消息" value={draft} onChange={event=>setDraft(event.target.value)}
                onKeyDown={event=>{if(event.nativeEvent.isComposing)return;if(event.key==='Escape'){event.preventDefault();cancel();}if(event.key==='Enter'&&(event.ctrlKey||event.metaKey)){event.preventDefault();apply();}}} />
              <div className="edit-toolbar"><span className="edit-key-hint">Esc 取消</span><div className="edit-actions">
                <TintButton data-editor-action type="button" onClick={cancel}>取消</TintButton>
                <TintButton data-editor-action type="button" className="font-medium" disabled={!draft.trim()} onClick={apply}>重新发送</TintButton>
              </div></div>
            </div>
          </div>
          <div ref={actionsRef} className="bubble-action-row" style={{pointerEvents:open?'none':undefined}} aria-hidden={open}>
            <Button ref={editButton} variant="ghost" tabIndex={open?-1:0} className="edit-start" onClick={edit} aria-label="编辑示例消息">编辑</Button>
          </div>
        </div>
        <div className="demo-answer" data-demo-answer><p>{ANSWER}</p><div className="reply-meta"><span>17.4k tokens</span><span><Clock3 size={12}/>3.0s</span><Button variant="ghost" size="icon" aria-label="复制示例回答" onClick={()=>writeClipboardText(ANSWER).then(()=>notify('已复制示例回答'),()=>notify('请手动选择回答文本'))}><Copy size={14}/></Button></div></div>
      </div>
    </div>
    <div className="demo-composer-wrap"><InputSurface className="demo-composer"><div className="fake-placeholder">要求后续变更</div><div className="demo-composer-toolbar"><Button variant="ghost" size="icon" aria-label="添加文件（示例）" disabled><Plus size={17}/></Button><span className="menu-reference-label">原版菜单对照<ChevronRight size={12}/></span><ModelThinkingSelector open={modelOpen} onOpenChange={setModelOpen} models={['deepseek-flash','deepseek-pro']} selectedModel={model} selectedThinking={thinking} thinkingOptions={THINKING} selectModelLabel="选择模型" noModelsLabel="暂无模型" onChooseModel={id=>{setModel(id);setModelOpen(false);}} onChooseThinking={setThinking}/><Button variant="secondary" size="icon" aria-label="发送（示例）" disabled><ArrowUp size={17}/></Button></div></InputSurface></div>
  </section>;
}

function App() {
  const [selected, setSelected] = useState<PlanId>('A'), [replay, setReplay] = useState(0);
  const [long, setLong] = useState(false), [slow, setSlow] = useState(false), [dark, setDark] = useState(false);
  const [diy, setDiy] = useState<Settings>(PLANS[5].settings), [toast, setToast] = useState('');
  const toastTimer = useRef<ReturnType<typeof setTimeout>>();
  const notify = (text: string) => { clearTimeout(toastTimer.current); setToast(text); toastTimer.current=setTimeout(()=>setToast(''),2300); };
  const plan = PLANS.find(p=>p.id===selected)!;
  const settings = selected==='F' ? diy : plan.settings;
  const result = useMemo(()=>`我选择方案 ${selected}：${plan.name}。${selected==='F'?`参数：向${diy.direction==='up'?'上':'下'}展开，宽度 ${diy.width}%，速度系数 ${diy.speed.toFixed(2)}，高度延迟 ${diy.delay}ms，内容模糊 ${diy.blur}px。`:'保留当前模型二级菜单的气泡形变、淡入和弹簧回收。'}`, [selected,plan,diy]);
  useEffect(()=>{ document.documentElement.dataset.previewTheme=dark?'dark':'light'; },[dark]);
  useEffect(()=>()=>clearTimeout(toastTimer.current),[]);
  const choose = (id:PlanId) => {setSelected(id);setReplay(value=>value+1);};
  const change = <K extends keyof Settings>(key:K,value:Settings[K])=>setDiy(previous=>({...previous,[key]:value}));
  return <main className="choice-page">
    <header className="page-heading"><div><div className="eyebrow"><span/>DEEPAGENT STUDIO <span className="eyebrow-divider">/</span> 交互预览</div><h1>消息编辑，保留气泡的变化</h1><p>沿用当前模型二级菜单的动效。试着编辑、取消，再选一种展开方式。</p></div><Button variant="ghost" className="theme-toggle" onClick={()=>setDark(value=>!value)} aria-label={dark?'切换浅色预览':'切换深色预览'}>{dark?<Sun size={17}/>:<Moon size={17}/>}<span>{dark?'浅色':'深色'}</span></Button></header>
    <div className="choice-layout"><aside className="plan-list" aria-label="六种编辑动效方案">
      {PLANS.map(item=><Button key={item.id} variant="ghost" className={`plan-card ${item.id===selected?'is-selected':''}`} aria-pressed={item.id===selected} aria-label={`选择方案 ${item.id}：${item.name}`} onClick={()=>choose(item.id)}>
        <span className="plan-letter">{item.id}</span><span className="plan-info"><strong>{item.name}</strong><span>{item.description}</span><small>{item.tag}</small></span>{item.id===selected&&<Check size={16} className="selection-check"/>}
      </Button>)}
    </aside><section className="preview-column"><div className="preview-controls"><div className="active-plan"><span>{selected}</span><strong>{plan.name}</strong></div><div className="play-tools"><Button variant="ghost" onClick={()=>setLong(value=>!value)} aria-pressed={long}>{long?'短消息':'长消息'}</Button><Button variant="ghost" aria-pressed={slow} onClick={()=>setSlow(value=>!value)}>{slow?'正常速度':'慢速观察'}</Button><Button variant="secondary" onClick={()=>setReplay(value=>value+1)}><Play size={13}/>重播</Button></div></div>
      <EditStage key={`${selected}-${long}-${replay}`} plan={selected} settings={settings} seed={long?LONG:SHORT} slow={slow} autoPlay={replay>0} notify={notify}/>
      <div className="stage-caption"><span><i/>{settings.direction==='up'?'编辑框向上展开，优先保留回答位置':'编辑框与回答一起平滑让位'}</span><span>下方模型按钮为项目现有组件</span></div>
      {selected==='F'&&<section className="diy-panel" aria-label="DIY 动效参数"><div className="diy-heading"><strong>调到你想要的手感</strong><span>改动实时生效，复制结果会带上参数</span></div><div className="diy-grid">
        <label>展开方向<select aria-label="展开方向" value={diy.direction} onChange={event=>change('direction',event.target.value as Settings['direction'])}><option value="down">向下展开 · 回答随动</option><option value="up">向上展开 · 下沿锚定</option></select></label>
        <label>编辑框宽度 <output>{diy.width}%</output><input aria-label="编辑框宽度" type="range" min="60" max="100" step="1" value={diy.width} onChange={event=>change('width',+event.target.value)}/></label>
        <label>速度系数 <output>{diy.speed.toFixed(2)}</output><input aria-label="速度系数" type="range" min="0.55" max="1.5" step="0.05" value={diy.speed} onChange={event=>change('speed',+event.target.value)}/></label>
        <label>高度延迟 <output>{diy.delay}ms</output><input aria-label="高度延迟" type="range" min="0" max="240" step="5" value={diy.delay} onChange={event=>change('delay',+event.target.value)}/></label>
        <label>内容模糊 <output>{diy.blur}px</output><input aria-label="内容模糊" type="range" min="0" max="6" step="0.5" value={diy.blur} onChange={event=>change('blur',+event.target.value)}/></label>
      </div></section>}
    </section></div>
    <footer className="choice-footer"><div className="footer-summary"><span className="footer-letter">{selected}</span><div><strong>已选 {selected} · {plan.name}</strong><p>只在本页预览，正式气泡待你选定后接入</p></div></div><Button onClick={()=>writeClipboardText(result).then(()=>notify('已复制选择结果，粘贴回对话即可'),()=>notify('自动复制未成功，请手动复制下方文字'))}><Copy size={14}/>复制选择结果</Button><textarea id="choice-result" className="choice-result" aria-label="选择结果" readOnly rows={selected==='F'?3:2} value={result}/></footer>
    <div className={`preview-toast ${toast?'visible':''}`} role="status" aria-live="polite">{toast}</div>
    {prefersReducedMotion()&&<p className="reduced-note">系统已启用减少动态效果，当前使用即时切换</p>}
  </main>;
}
createRoot(document.getElementById('root')!).render(<App/>);
