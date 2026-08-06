const CACHE="session-board-v2";
self.addEventListener("install",e=>e.waitUntil(caches.open(CACHE).then(c=>c.addAll(["/","/styles.css","/assets/app.js","/manifest.webmanifest"]))));
self.addEventListener("activate",e=>e.waitUntil(caches.keys().then(keys=>Promise.all(keys.filter(key=>key!==CACHE).map(key=>caches.delete(key))))));
self.addEventListener("fetch",e=>{if(e.request.method==="GET"&&new URL(e.request.url).origin===location.origin)e.respondWith(fetch(e.request).catch(()=>caches.match(e.request)))});
self.addEventListener("push",e=>{
  let message={};
  try{message=e.data?.json()??{}}catch{}
  const title=typeof message.title==="string"?message.title:"Session Board approval needed";
  const body=typeof message.body==="string"?message.body:"A session is waiting for your decision.";
  e.waitUntil(self.registration.showNotification(title,{body,icon:"/icon.svg",badge:"/icon.svg",data:{url:"/"},tag:typeof message.actionId==="string"?`action:${message.actionId}`:"session-board-action"}));
});
self.addEventListener("notificationclick",e=>{
  e.notification.close();
  e.waitUntil(clients.matchAll({type:"window",includeUncontrolled:true}).then(windows=>{
    const board=windows.find(client=>new URL(client.url).origin===location.origin);
    return board?board.focus():clients.openWindow(e.notification.data?.url??"/");
  }));
});
