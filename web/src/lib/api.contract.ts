import searchResultFixture from "./search-result.contract.json";
import type { SearchResult } from "./api";

if (searchResultFixture.protocol !== "torrent" && searchResultFixture.protocol !== "usenet") {
  throw new Error("Invalid search fixture release protocol");
}
const protocol:SearchResult["protocol"]=searchResultFixture.protocol;
const searchResultContract: SearchResult = {...searchResultFixture, protocol};

void searchResultContract;
