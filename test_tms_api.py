import requests, re, json

main_url = "https://maplestory.beanfun.com/main"
list_url = "https://maplestory.beanfun.com/main?handler=BulletinProxy"
detail_url = "https://maplestory.beanfun.com/bulletin?handler=BulletinDetail"

s = requests.Session()
r = s.get(main_url)
m = re.search(r'name="__RequestVerificationToken"[^>]*value="([^"]+)"', r.text)
csrf = m.group(1)
h = {"X-CSRF-Token": csrf}

# Get list - check all available keys
r = s.post(list_url, data={"Kind": "0", "Page": "1", "method": "0", "PageSize": "3"}, headers=h)
items = r.json()["data"]["myDataSet"]["table"]
print("=== Bulletin list item keys ===")
if items:
    print(f"Fields: {list(items[0].keys())}")
    for item in items:
        for k, v in item.items():
            if isinstance(v, str) and v.strip():
                print(f"  {k}: {v[:80]}")

# Search for maintenance
print("\n=== Searching for maintenance ===")
r = s.post(list_url, data={"Kind": "0", "Page": "1", "method": "0", "PageSize": "20"}, headers=h)
items = r.json()["data"]["myDataSet"]["table"]
test_bid = None
for item in items:
    title = item.get("bullentinName", "")
    if u"\u7ef4\u62a4" in title or u"\u505c\u673a" in title or u"\u505c\u6a5f" in title:
        test_bid = item["bullentinId"]
        print(f"Found maintenance: #{test_bid} ({title[:60]})")
        break

if not test_bid:
    test_bid = items[0]["bullentinId"]
    print(f"No maintenance on page 1, using first item: #{test_bid}")

# Get detail
print("\n=== Bulletin detail ===")
r = s.post(detail_url, data={"Bid": test_bid}, headers=h)
d = r.json()
print(f"Top keys: {list(d.keys())}")
print(f"code={d.get('code')}, message={d.get('message')}")
tbl = d["data"]["myDataSet"]["table"]
if isinstance(tbl, list) and tbl:
    row = tbl[0]
    print(f"\nDetail fields ({len(row)}):")
    for k, v in sorted(row.items()):
        if isinstance(v, str):
            if len(v) > 120:
                print(f"  {k}: [{len(v)} chars] {v[:120]}...")
            else:
                print(f"  {k}: {v}")
        elif v is None:
            print(f"  {k}: null")
        else:
            print(f"  {k}: {v}")
