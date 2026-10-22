(() => {
  const specs = [
    {id:'A',name:'圆珠',subtitle:'26 × 26 · 圆形',w:26,h:26,rail:30,duration:500,ease:'reference',boost:1,desc:'贴合轨道，轻柔滑行'},
    {id:'B',name:'横椭圆',subtitle:'36 × 22 · 横向椭圆',w:36,h:22,rail:28,duration:620,ease:'sine.inOut',boost:1.1,desc:'低矮柔和，起步和落点都放慢'},
    {id:'C',name:'竖椭圆',subtitle:'20 × 30 · 竖向椭圆',w:20,h:30,rail:34,duration:440,ease:'power2.inOut',boost:.85,desc:'窄身清晰，切换干净利落'},
    {id:'D',name:'悬浮圆珠',subtitle:'34 × 34 · 细轨圆珠',w:34,h:34,rail:14,duration:560,ease:'back.out(1.15)',boost:1.25,desc:'圆珠浮在细轨上，落点轻轻回弹'},
    {id:'E',name:'长胶囊',subtitle:'44 × 24 · 胶囊形',w:44,h:24,rail:32,duration:680,ease:'reference',boost:1.4,desc:'接触面宽，移动更从容'},
    {id:'F',name:'自己调',subtitle:'28 × 28 · 自定义圆 / 椭圆',w:28,h:28,rail:32,duration:550,ease:'reference',boost:1.2,desc:'尺寸、滑行时长、拖动加速可调'},
  ];
  const SPRING=[[0,0],[.02,.015],[.04,.0532],[.06,.1065],[.08,.1686],[.1,.2351],[.13,.3363],[.16,.4332],[.2,.5495],[.24,.648],[.28,.7287],[.33,.807],[.38,.8646],[.44,.9128],[.5,.9447],[.57,.968],[.65,.9834],[.74,.9924],[.85,.9975],[1,1]];
  const referenceEase=p=>{const i=SPRING.findIndex(([x])=>x>=p);if(i<=0)return i===0?0:1;const[a,b]=SPRING[i-1],[c,d]=SPRING[i];return b+(d-b)*(p-a)/(c-a);};
  const names=['简单','中度','深度'];
  const grid=document.querySelector('.grid');
  const media=matchMedia('(prefers-reduced-motion: reduce)');
  let reduced=media.matches,slow=false,chosen=null,toastTween;
  const controllers=[];
  grid.innerHTML=specs.map(s=>`<article class="option kind-${s.id.toLowerCase()}" data-option="${s.id}" aria-labelledby="title-${s.id}">
    <div class="option-top"><div class="identity"><span class="letter">${s.id}</span><div><h2 id="title-${s.id}">${s.name}</h2><p class="shape-spec">${s.subtitle}</p></div></div><button class="select-option" type="button" aria-label="选择方案 ${s.id} ${s.name}" aria-pressed="false">✓</button></div>
    <div class="preview"><div class="effort" style="--thumb-w:${s.w}px;--thumb-h:${s.h}px;--track-h:${s.rail}px">
      <div class="effort-heading"><span>更快</span><span class="current"><span>中度</span></span><span>更深入</span></div>
      <button class="slider-hit" type="button" role="slider" aria-label="方案 ${s.id} 思考强度" aria-valuemin="0" aria-valuemax="2" aria-valuenow="1" aria-valuetext="中度"><span class="rail"><span class="field-mask"><canvas aria-hidden="true"></canvas></span><span class="dots" aria-hidden="true"><i></i><i></i><i></i></span></span><span class="thumb" aria-hidden="true"></span></button>
      <div class="stops">${names.map((n,i)=>`<button type="button" data-index="${i}" aria-label="方案 ${s.id} ${n}" aria-pressed="${i===1}">${n}</button>`).join('')}</div>
      <div class="motion-detail"><span class="motion-status">${s.duration} ms · 点击滑行</span><span class="speed" aria-label="粒子流速">流速 1.0×</span></div>
    </div></div>
    ${s.id==='F'?`<div class="diy">
      <label>滑块宽度 <span data-output="w">28 px</span><input aria-label="DIY 滑块宽度" data-param="w" type="range" min="18" max="48" value="28" step="1"></label>
      <label>滑块高度 <span data-output="h">28 px</span><input aria-label="DIY 滑块高度" data-param="h" type="range" min="18" max="36" value="28" step="1"></label>
      <label>滑行时长 <span data-output="duration">550 ms</span><input aria-label="DIY 滑行时长" data-param="duration" type="range" min="350" max="950" value="550" step="25"></label>
      <label>拖动加速 <span data-output="boost">1.2</span><input aria-label="DIY 拖动加速" data-param="boost" type="range" min="0" max="2" value="1.2" step=".1"></label>
    </div>`:''}
    <div class="card-bottom"><p class="description">${s.desc}</p><button class="replay" type="button" aria-label="演示方案 ${s.id}">演示一次 ↗</button></div>
  </article>`).join('');

  function copyText(){
    if(!chosen)return '';
    const c=controllers.find(c=>c.spec.id===chosen),s=c.spec;
    return `我选择方案 ${s.id}：${s.name}。滑块 ${s.w} × ${s.h} px，轨道 ${s.rail} px，点击滑行 ${s.duration} ms，拖动加速幅度 ${s.boost.toFixed(1)}。保留简单／中度／深度三档；点击平滑过渡，真实拖动才跟手，松手吸附，粒子随后减速。`;
  }
  function updateChoice(){
    const c=controllers.find(c=>c.spec.id===chosen);
    document.querySelector('#selection-title').textContent=c?`已选 ${c.spec.id} · ${c.spec.name}`:'先试手感，再选造型';
    document.querySelector('#selection-description').textContent=c?'复制下面这段选择结果发给我，即可按此接入':'直接点击档位或拖动滑块；点卡片右上角确定选择';
    const result=document.querySelector('#choice-result');result.value=copyText();result.classList.toggle('visible',!!c);
    document.querySelector('#copy-choice').disabled=!c;
  }
  function select(id){chosen=id;controllers.forEach(c=>{const active=c.spec.id===id;c.card.classList.toggle('selected',active);c.card.querySelector('.select-option').setAttribute('aria-pressed',String(active));});updateChoice();}
  function toast(text){const node=document.querySelector('.toast');node.textContent=text;toastTween?.kill();gsap.set(node,{opacity:1});toastTween=gsap.to(node,{opacity:0,duration:.2,delay:2});}
  class EffortPreview {
    constructor(spec){
      this.spec=spec;this.card=grid.querySelector(`[data-option="${spec.id}"]`);this.root=this.card.querySelector('.effort');this.hit=this.card.querySelector('.slider-hit');this.label=this.card.querySelector('.current span');
      this.state={ratio:.5,press:1,boost:1};this.target=.5;this.drag=null;this.lastLabel=1;this.timeline=null;
      this.field=createEffortField(this.card.querySelector('canvas'),()=>this.state.ratio,()=>this.state.boost);
      this.render();this.field.draw(0,true);
      this.hit.addEventListener('pointerdown',e=>this.down(e));this.hit.addEventListener('pointermove',e=>this.move(e));this.hit.addEventListener('pointerup',e=>this.up(e));
      this.hit.addEventListener('pointercancel',()=>this.cancel());this.hit.addEventListener('lostpointercapture',()=>{if(this.drag)this.cancel();});
      this.hit.addEventListener('keydown',e=>{let n=Math.round(this.target*2);if(['ArrowRight','ArrowUp'].includes(e.key))n++;else if(['ArrowLeft','ArrowDown'].includes(e.key))n--;else if(e.key==='Home')n=0;else if(e.key==='End')n=2;else return;e.preventDefault();select(spec.id);this.choose(n/2);});
      this.card.querySelectorAll('[data-index]').forEach(b=>b.addEventListener('click',()=>{select(spec.id);this.choose(Number(b.dataset.index)/2);}));
      this.card.querySelector('.select-option').addEventListener('click',()=>select(spec.id));this.card.querySelector('.replay').addEventListener('click',()=>this.replay());
      this.card.addEventListener('click',e=>{if(!e.target.closest('button,input,label'))select(spec.id);});
      this.card.querySelectorAll('[data-param]').forEach(input=>input.addEventListener('input',()=>{spec[input.dataset.param]=Number(input.value);this.syncSpec();select(spec.id);}));
    }
    ease(){return this.spec.ease==='reference'?referenceEase:this.spec.ease;}
    render(){
      const {ratio,press}=this.state;
      this.root.style.setProperty('--ratio',String(ratio));this.root.style.setProperty('--press',String(press));this.root.style.setProperty('--accent-hue',String(210+Math.max(0,Math.min(1,ratio))**2*63));
      const index=Math.max(0,Math.min(2,Math.round(ratio*2)));
      this.hit.setAttribute('aria-valuenow',String(index));this.hit.setAttribute('aria-valuetext',names[index]);
      this.card.querySelectorAll('[data-index]').forEach((b,i)=>b.setAttribute('aria-pressed',String(i===index)));
      if(this.lastLabel!==index){this.lastLabel=index;this.label.textContent=names[index];gsap.killTweensOf(this.label);if(!reduced)gsap.fromTo(this.label,{y:5,opacity:.3},{y:0,opacity:1,duration:.2,ease:'power2.out'});}
      this.root.querySelector('.dots').style.opacity=String(1-ratio*.75);
      this.root.querySelector('.speed').textContent=`流速 ${this.state.boost.toFixed(1)}×`;
      if(reduced)this.field.draw(0,true);
    }
    position(clientX){const r=this.hit.getBoundingClientRect(),inset=this.spec.w/2+3;return Math.max(0,Math.min(1,(clientX-r.left-inset)/Math.max(1,r.width-inset*2)));}
    choose(ratio){
      this.timeline?.kill();this.target=Math.round(Math.max(0,Math.min(1,ratio))*2)/2;
      gsap.to(this.state,{ratio:this.target,duration:reduced?0:this.spec.duration/1000*(slow?1.8:1),ease:this.ease(),overwrite:'auto',onUpdate:()=>this.render()});
    }
    down(e){
      if(e.button!==0||!e.isPrimary)return;e.preventDefault();this.hit.focus({preventScroll:true});select(this.spec.id);this.timeline?.kill();
      this.drag={id:e.pointerId,startX:e.clientX,lastX:e.clientX,time:e.timeStamp,moved:false,before:this.target};this.hit.setPointerCapture(e.pointerId);
      gsap.to(this.state,{press:.94,duration:reduced?0:.09,overwrite:'auto',onUpdate:()=>this.render()});
      // A press is not a drag: position remains unchanged until movement crosses 4 px.
    }
    move(e){
      const d=this.drag;if(!d||d.id!==e.pointerId)return;
      if(!d.moved&&Math.abs(e.clientX-d.startX)<4)return;d.moved=true;this.hit.classList.add('dragging');
      const velocity=Math.abs(e.clientX-d.lastX)/Math.max(8,e.timeStamp-d.time);d.lastX=e.clientX;d.time=e.timeStamp;
      gsap.killTweensOf(this.state,'ratio');this.state.ratio=this.position(e.clientX);this.target=this.state.ratio;
      const boost=1+this.spec.boost*(.55+Math.min(1.75,velocity*.5));
      gsap.to(this.state,{boost:reduced?1:boost,duration:.12,ease:'power2.out',overwrite:'auto',onUpdate:()=>this.render()});
      this.root.querySelector('.motion-status').textContent='拖动跟手 · 粒子提速';this.render();
    }
    up(e){
      const d=this.drag;if(!d||d.id!==e.pointerId)return;this.drag=null;this.hit.classList.remove('dragging');this.hit.releasePointerCapture(e.pointerId);
      this.choose(this.position(e.clientX));this.release();
    }
    release(){gsap.to(this.state,{press:1,boost:1,duration:reduced?0:.7,ease:referenceEase,overwrite:'auto',onUpdate:()=>this.render()});this.root.querySelector('.motion-status').textContent=`${this.spec.duration} ms · 点击滑行`;}
    cancel(){if(!this.drag)return;const previous=this.drag.before;this.drag=null;this.hit.classList.remove('dragging');this.choose(previous);this.release();}
    replay(){
      this.timeline?.kill();gsap.killTweensOf(this.state);this.state.ratio=0;this.target=0;this.render();
      if(reduced){this.choose(1);return;}
      const d=this.spec.duration/1000*(slow?1.8:1);this.timeline=gsap.timeline({onUpdate:()=>this.render()})
        .to(this.state,{ratio:1,duration:d,ease:this.ease()})
        .to(this.state,{ratio:.5,duration:d,ease:this.ease()},'>.32');
      this.target=.5;
    }
    syncSpec(){const s=this.spec;this.root.style.setProperty('--thumb-w',`${s.w}px`);this.root.style.setProperty('--thumb-h',`${s.h}px`);this.root.style.setProperty('--track-h',`${Math.max(30,s.h+4)}px`);s.rail=Math.max(30,s.h+4);
      this.card.querySelector('.shape-spec').textContent=`${s.w} × ${s.h} · ${s.w===s.h?'圆形':'椭圆形'}`;
      this.card.querySelectorAll('[data-output]').forEach(o=>{const k=o.dataset.output;o.textContent=k==='duration'?`${s[k]} ms`:k==='boost'?s[k].toFixed(1):`${s[k]} px`;});
      this.root.querySelector('.motion-status').textContent=`${s.duration} ms · 点击滑行`;this.render();updateChoice();
    }
  }
  specs.forEach(spec=>controllers.push(new EffortPreview(spec)));
  const drawAll=time=>{if(document.hidden)return;controllers.forEach(c=>{c.field.draw(time,reduced);c.render();});};
  if(!reduced)gsap.ticker.add(drawAll);
  media.addEventListener('change',()=>{reduced=media.matches;gsap.ticker.remove(drawAll);controllers.forEach(c=>{c.timeline?.kill();gsap.killTweensOf(c.state);Object.assign(c.state,{ratio:c.target,press:1,boost:1});c.render();c.field.draw(0,true);});if(!reduced)gsap.ticker.add(drawAll);document.querySelector('#reduced-note').hidden=!reduced;});
  document.querySelector('#reduced-note').hidden=!reduced;
  document.querySelector('#play-all').addEventListener('click',()=>controllers.forEach(c=>c.replay()));
  document.querySelector('#slow').addEventListener('click',e=>{slow=!slow;e.currentTarget.setAttribute('aria-pressed',String(slow));});
  document.querySelector('#theme').addEventListener('click',e=>{const dark=document.body.classList.toggle('dark');e.currentTarget.textContent=dark?'浅色预览':'深色预览';e.currentTarget.setAttribute('aria-pressed',String(dark));});
  document.querySelector('#copy-choice').addEventListener('click',async()=>{
    try{await writeClipboardText(copyText());toast('已复制，发回对话即可');}
    catch{document.querySelector('#choice-result').focus();document.querySelector('#choice-result').select();toast('请按 Ctrl+C 复制已选中的文字');}
  });
  window.addEventListener('pagehide',()=>{gsap.ticker.remove(drawAll);controllers.forEach(c=>{c.timeline?.kill();gsap.killTweensOf(c.state);c.field.dispose();});},{once:true});
  // Exposed only in this review file for measurable trajectory/acceleration checks.
  window.effortChoices={controllers,select,getResult:copyText};
})();
