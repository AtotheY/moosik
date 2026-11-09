
const screen = document.querySelector('.screen');
const source = document.querySelector('source');
const details = document.querySelector('output');
const tabs = document.querySelectorAll('[data-view]');
for (const tab of tabs) {
  tab.addEventListener('click', () => {
    const name = tab.dataset.view;
    details.hidden = true;
    panel.hidden = name !== 'records';
    if (!panel.hidden) showCase();
    source.srcset = name + '-narrow.png';
    screen.src = name + '.png';
    screen.alt = 'Moosik Collection, ' + tab.getAttribute('aria-label') + ' tab';
    for (const other of tabs) other.setAttribute('aria-pressed', String(tab === other));
  });
}
const dialog = document.querySelector('dialog');
const turntable = dialog.querySelector('.turntable');
let yaw = 0;
let pitch = 1;
let origin = null;
function paintFrame() {
  if (!model.hidden) {
    model.style.transform = 'rotateX(' + ((pitch-1)*24-6) + 'deg) rotateY(' + (yaw*11.25-17) + 'deg)';
    return;
  }
  const column = (Math.round(yaw) % 32 + 32) % 32;
  const row = Math.round(pitch)*4 + Math.floor(column / 8);
  turntable.style.backgroundPosition = (column%8)*100/7 + '% ' + row*100/11 + '%';
}
dialog.querySelector('button').onclick=()=>dialog.close();
dialog.addEventListener('click', e=>{
  const r=dialog.getBoundingClientRect();
  if(e.target===dialog && (e.clientX<r.left || e.clientX>r.right || e.clientY<r.top || e.clientY>r.bottom))dialog.close();
});
turntable.addEventListener('pointerdown', e=>{
  origin=[e.clientX,e.clientY,yaw,pitch];
  turntable.setPointerCapture(e.pointerId);
});
turntable.addEventListener('pointermove',e=>{
  if(!origin) return;
  yaw=origin[2]+(e.clientX-origin[0])/12;
  pitch=Math.max(0,Math.min(2,origin[3]-(e.clientY-origin[1])/36));
  paintFrame();
});
turntable.addEventListener('lostpointercapture',()=>{origin=null});
turntable.addEventListener('keydown',e=>{
  if(!['ArrowLeft','ArrowRight','ArrowUp','ArrowDown'].includes(e.key)) return;
  e.preventDefault();
  if(e.key==='ArrowLeft') yaw--;
  if(e.key==='ArrowRight') yaw++;
  if(e.key==='ArrowUp') pitch=Math.min(2,pitch+1);
  if(e.key==='ArrowDown') pitch=Math.max(0,pitch-1);
  paintFrame();
});
const ticket = '<small>ARTIST PASS　/　BACKSTAGE</small><strong>Drake</strong><div class="measures"><div><b>12</b><small>Songs heard</small></div><div><b>33</b><small>Listening days</small></div><div><b>13h 11m</b><small>Time listened</small></div></div><small class="next">Next stamp · All Access in 27 listening days</small>';
const song = '<b>Gold</b><span>112 full listens · 5h 36m listened</span><span class="secondary">138 listens to Platinum · ≈6h 54m</span>';
for (const button of document.querySelectorAll('[data-hover]')) {
  function reveal() {
    const isTicket=button.dataset.hover==='ticket';
    details.className=isTicket?'ticket-detail':'song-detail';
    details.innerHTML=isTicket?ticket:song;
    details.hidden=false;
  }
  button.addEventListener('pointerenter',reveal);
  button.addEventListener('focus',reveal);
  button.addEventListener('click',reveal);
  button.addEventListener('pointerleave',() => {details.hidden = true});
  button.addEventListener('blur',() => {details.hidden = true});
}
document.addEventListener('keydown',event => {if(event.key === 'Escape') details.hidden = true});
document.addEventListener('pointerdown',event => {if(!event.target.closest('[data-hover]')) details.hidden = true});
const panel = document.querySelector('.panel');
const model = dialog.querySelector('.model');
const songs = [];
function add(title, artist, listens, minutes, art, tier, rotation) {
  songs.push({title, artist, listens, minutes, art, tier, rotation});
}
add('Classic PT2','Drake',112,336,0,'Gold','gold');
add('DISRESPECTFUL','Doja Cat',1008,2419,1,'Diamond','diamond');
add('In The Bible (feat. Lil Durk & Giveon)','Drake',4,29,2);
add('Hours In Silence','Drake',32,206,3,'Bronze');
add('Calling For You (feat. 21 Savage)','Drake',28,138,4,'Bronze');
add('Pipe Down','Drake',9,33,2);
add('Rain at Night','Field Recordings',1,600);
for (const title of ['Fair Trade','Champagne Poetry','Race My Mind','Fountains','Knife Talk','The Remorse','TSU']) add(title,'Drake',2,7,2);
for (let i=1; i<=266; i++) add('Session '+String(i).padStart(3,'0'),'Studio sessions',1,1);
const ordered = [...songs].sort((a,b) => a.artist.localeCompare(b.artist) || a.title.localeCompare(b.title));
const metals = {Diamond:'#9ae3f5', Gold:'#efc369', Bronze:'#d89061'};
function openSleeve(song) {
  const {title, artist, listens, minutes, tier, rotation, art} = song;
  dialog.querySelector('.tier').textContent = tier ? tier+' record' : 'Toward Iron';
  dialog.querySelector('h2').textContent = title;
  dialog.querySelector('.artist').textContent = artist;
  const duration = minutes >= 60 ? Math.floor(minutes/60)+'h '+String(minutes%60).padStart(2,'0')+'m' : minutes+'m';
  dialog.querySelector('.totals').textContent = listens+' full '+(listens===1?'listen':'listens')+' · '+duration+' listened';
  dialog.style.setProperty('--metal', metals[tier] || '#91a3ae');
  turntable.style.backgroundImage = rotation ? 'url('+rotation+'-rotation.webp)' : 'none';
  model.hidden = Boolean(rotation);
  model.querySelector('.front').style.backgroundImage = art == null ? 'none' : 'url(covers/'+art+'.webp)';
  model.querySelector('.gem').textContent = tier ? '✦' : '';
  model.querySelector('.back').textContent = title+' · '+artist;
  yaw=0; pitch=1; paintFrame();
  dialog.showModal(); turntable.focus();
}
function button(text, cls, click) {
  const b = document.createElement('button');
  b.className=cls; b.textContent=text; b.onclick=click;
  return b;
}
function picture(src) {
  const img = new Image(); img.src=src; img.alt=''; return img;
}
function heading(title, back) {
  panel.replaceChildren(); panel.scrollTop=0;
  const head=document.createElement('div'); head.className='heading';
  if (back) {
    const b=button('‹','icon',showCase); b.setAttribute('aria-label','Back to record crates'); head.append(b);
  }
  const h=document.createElement('h2'); h.textContent=title; head.append(h); panel.append(head);
  return head;
}
function showCase() {
  const head=heading('Record crates');
  const find=button('','find icon',showFind);
  find.setAttribute('aria-label','Find a record'); find.innerHTML='<i class="glass"></i>'; head.append(find);
  const grid=document.createElement('div'); grid.className='crates'; panel.append(grid);
  for (const rank of ['Diamond','Gold','Bronze']) {
    const group=ordered.filter(s=>s.tier===rank);
    const b=button('','',()=>group.length===1 ? openSleeve(group[0]) : showCrate(rank,group));
    b.setAttribute('aria-label',rank+' crate, '+group.length+' '+(group.length===1?'record':'records')); b.dataset.album=rank.toLowerCase();
    const count=document.createElement('small'); count.textContent=group.length+(group.length===1?' record':' records');
    b.append(picture('covers/crate-'+rank.toLowerCase()+'.webp'),count); grid.append(b);
  }
  const h=document.createElement('h2'); h.className='next-title'; h.textContent='Next milestone';
  const pipe=songs[5], next=button('','milestone',()=>openSleeve(pipe));
  next.append(picture('covers/'+pipe.art+'.webp'));
  const info=document.createElement('span'); info.innerHTML='<b>Pipe Down</b><small>1 listen to Iron</small><i></i>'; next.append(info);
  panel.append(h,next);
}
function showCrate(rank, group) {
  heading(rank+' crate',true);
  const grid=document.createElement('div'); grid.className='sleeves'; panel.append(grid);
  for (const song of group) {
    const b=button('','',()=>openSleeve(song));
    const title=document.createElement('b'); title.textContent=song.title;
    const by=document.createElement('small'); by.textContent=song.artist;
    b.append(picture('covers/sleeve-'+song.art+'.webp'),title,by); grid.append(b);
  }
}
function showFind() {
  heading('Find a record',true);
  const input=document.createElement('input'); input.type='search'; input.placeholder='Find a song or artist';
  input.setAttribute('aria-label',input.placeholder);
  const filters=document.createElement('div'); filters.className='filters';
  const results=document.createElement('div');
  let status='All';
  function update() {
    results.replaceChildren();
    const query=input.value.trim().toLowerCase();
    for (const filter of filters.children) filter.setAttribute('aria-pressed', String(filter.textContent===status));
    const matches=ordered.filter(s=>(status==='All' || Boolean(s.tier)===(status==='Earned')) && (s.title.toLowerCase().includes(query) || s.artist.toLowerCase().includes(query)));
    for (const song of matches) {
      const b=button('','result',()=>openSleeve(song));
      if (song.art!=null) b.append(picture('covers/'+song.art+'.webp'));
      else {const icon=document.createElement('i'); icon.className='placeholder'; icon.textContent='♫'; b.append(icon);}
      const name=document.createElement('span'), title=document.createElement('b'), by=document.createElement('small');
      title.textContent=song.title; by.textContent=song.artist; name.append(title,by);
      const rank=document.createElement('em'); rank.textContent=song.tier || 'Toward Iron';
      b.append(name,rank); results.append(b);
    }
    if (!matches.length) results.textContent='No matching records';
  }
  for (const label of ['All','Earned','In progress']) filters.append(button(label,'',()=>{status=label;update()}));
  input.addEventListener('input',update);
  panel.append(input,filters,results); update(); input.focus();
}
showCase();
