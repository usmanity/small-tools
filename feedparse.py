import feedparser

rss_url = 'https://blog.usmanity.com/tag/75-day-challenge/rss'
feed = feedparser.parse(rss_url)

markdown_output = ""
for entry in feed.entries:
    markdown_link = f"- [{entry.title}]({entry.link})"    
    markdown_output += f"{markdown_link}\n"

print(markdown_output)
