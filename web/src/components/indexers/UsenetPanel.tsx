import {type FormEvent,useEffect,useState} from "react";
import {fetchUsenetSources,saveUsenetSource,deleteUsenetSource,
  type UsenetSource,ApiError} from "../../lib/api";

export function UsenetPanel({
  token,onUnauthorized,onChanged,
}:{
  token:string;onUnauthorized:()=>void;onChanged:()=>void;
}){
  const [sources,setSources]=useState<UsenetSource[]>([]);
  const [id,setId]=useState("usenet-");
  const [name,setName]=useState("");
  const [endpoint,setEndpoint]=useState("");
  const [apiKey,setApiKey]=useState("");
  const [enabled,setEnabled]=useState(true);
  const [busy,setBusy]=useState(false);
  const [notice,setNotice]=useState<string|null>(null);
  async function refresh(){
    setSources(await fetchUsenetSources(token));
  }
  useEffect(()=>{
    let active=true;
    void fetchUsenetSources(token).then(rows=>{if(active)setSources(rows);})
      .catch(error=>{
        if(!active)return;
        if(error instanceof ApiError&&error.status===401)onUnauthorized();
        else setNotice(error instanceof Error?error.message:"Unable to load Usenet sources");
      });
    return ()=>{active=false;};
  },[token,onUnauthorized]);

  function edit(source:UsenetSource){
    setId(source.id);setName(source.name);setEndpoint(source.endpoint);
    setEnabled(source.enabled);setApiKey("");setNotice(null);
  }
  async function submit(event:FormEvent<HTMLFormElement>){
    event.preventDefault();if(busy)return;
    setBusy(true);setNotice(null);
    try {
      await saveUsenetSource(token,id,{name,endpoint,enabled,
        ...(apiKey?{apiKey}:{})});
      await refresh();onChanged();setApiKey("");
      setNotice("Newznab source saved. API key is encrypted.");
    } catch(error) {
      if(error instanceof ApiError&&error.status===401)onUnauthorized();
      else setNotice(error instanceof Error?error.message:"Failed to save Newznab source");
    } finally {setBusy(false);}
  }
  async function remove(source:UsenetSource){
    if(busy)return;
    setBusy(true);setNotice(null);
    try{
      await deleteUsenetSource(token,source.id);
      await refresh();onChanged();
      if(id===source.id){setId("usenet-");setName("");setEndpoint("");setApiKey("");}
      setNotice("Newznab source deleted.");
    }catch(error){
      setNotice(error instanceof Error?error.message:"Failed to remove Newznab source");
    }finally{setBusy(false);}
  }
  return <section className="data-section" aria-label="Usenet/Newznab indexers">
    <div className="section-heading"><div>
      <h2>Usenet / Newznab</h2>
      <p className="muted">Manage HTTPS NZB search sources. API keys are stored encrypted and never returned to the browser.</p>
    </div></div>
    {notice&&<p className="notice" role="status">{notice}</p>}
    <div className="table-frame"><table><thead><tr>
      <th>Source</th><th>Endpoint</th><th>Status</th><th>Actions</th>
    </tr></thead><tbody>
      {sources.map(source=><tr key={source.id}>
        <td><strong>{source.name}</strong><small>{source.id}</small></td>
        <td>{source.endpoint}</td>
        <td>{source.enabled?"Enabled":"Disabled"} · {source.keyConfigured?"Key configured":"Key missing"}</td>
        <td><button className="quiet-button" type="button" onClick={()=>edit(source)}>Edit</button>
        <button className="quiet-button" type="button" disabled={busy}
          onClick={()=>void remove(source)}>Remove</button></td>
      </tr>)}
      {sources.length===0&&<tr><td colSpan={4}>No Newznab indexers configured.</td></tr>}
    </tbody></table></div>
    <form className="settings-form" onSubmit={event=>void submit(event)}>
      <h3>Add or edit Newznab source</h3>
      <div className="advanced-search-grid">
        <label>Source identifier <input required maxLength={100} pattern="usenet-[a-z0-9-]+"
          value={id} onChange={event=>setId(event.target.value)} /></label>
        <label>Display name <input required maxLength={100} value={name}
          onChange={event=>setName(event.target.value)} /></label>
        <label>HTTPS endpoint <input required type="url" value={endpoint}
          placeholder="https://usenet.example/api" onChange={event=>setEndpoint(event.target.value)}/></label>
        <label>API key (leave blank to keep current) <input type="password" autoComplete="off"
          value={apiKey} maxLength={512} onChange={event=>setApiKey(event.target.value)}/></label>
        <label className="checkbox-field"><input type="checkbox" checked={enabled}
          onChange={event=>setEnabled(event.target.checked)}/>Enabled</label>
      </div>
      <button className="primary-button" disabled={busy} type="submit">
        {busy?"Saving…":"Save Newznab source"}</button>
    </form>
  </section>;
}
