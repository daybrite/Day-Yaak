# Yaak for Day: UI strings (https://daybrite.dev/docs/localization). Add a locale by dropping a
# sibling folder (e.g. locales/fr/app.ftl) and translating.
#
# The appearance and language rows on the Settings page label themselves from Day's own catalog,
# so there are no keys for them here.

app_title = Yaak

nav_settings = Settings

menu_file = File
menu_edit = Edit
menu_request = Request

cmd_new_request = New Request
cmd_delete = Delete
cmd_send = Send
cmd_cancel = Cancel
cmd_layout = Switch layout
cmd_layout_stack = Put the response under the request
cmd_layout_beside = Put the response beside the request
cmd_add = Add
cmd_choose_file = Choose File…
cmd_remove_file = Remove File
cmd_remove_row = Remove
cmd_clear_history = Clear History
confirm_clear_history = Clear this request's response history?
confirm_clear_history_detail = Its stored responses and their bodies are deleted.

field_name = Name
field_name_hint = Untitled request
field_environment = Environment
environment_base = Base environment only
url_hint = https://example.com/api

tab_body = Body
tab_params = Params
tab_headers = Headers
tab_auth = Auth
tab_settings = Settings

pair_name_hint = Name
pair_value_hint = Value

body_none = No Body
body_json = JSON
body_text = Text
body_xml = XML
body_form = Form (URL-encoded)
body_multipart = Form (multipart)
body_graphql = GraphQL
body_binary = Binary File
body_hint = Request body
graphql_query = Query
graphql_query_hint = query {"{"} … {"}"}
graphql_variables = Variables
graphql_variables_hint = JSON variables
binary_hint = Path of the file to send
params_hint = Each parameter is added to the URL's query. A name that starts with a colon, like :id, fills that placeholder in the URL's path.

auth_inherit = Inherit
auth_none = No Authentication
auth_basic = Basic
auth_bearer = Bearer Token
auth_apikey = API Key
auth_inherit_hint = The request uses the authentication of its folder or workspace.
auth_foreign_hint = { $name } authentication comes from a Yaak plugin, which this client does not run. It is kept as stored, and a send reports that it cannot be applied.
auth_username = Username
auth_password = Password
auth_token = Token
auth_prefix = Prefix
auth_location = Send as
auth_location_header = Header
auth_location_query = Query parameter
auth_key_header = Header name
auth_key_query = Parameter name
auth_value = API key
auth_show = Show secrets
auth_enabled = Enabled

setting_inherit = Inherit
setting_on = On
setting_off = Off
setting_follow_redirects = Follow redirects
setting_validate_certificates = Validate certificates
setting_send_cookies = Send cookies
setting_store_cookies = Store cookies
setting_http_version = HTTP version
http_version_auto = Automatic
http_version_1 = HTTP/1.1
http_version_2 = HTTP/2
setting_timeout = Timeout (ms)
setting_timeout_hint = Inherit
settings_hint = A setting left on Inherit takes its value from the folder, then the workspace.

pane_request = Request
pane_response = Response

empty_workspace = This workspace has no requests yet. Add one with New Request.
empty_selection = Choose a request, or add one with New Request.

response_none = Send the request to see its response here.
response_sending = Sending…
response_failed = Request failed
response_tab_body = Body
response_tab_headers = Headers
response_tab_timeline = Timeline
section_response_headers = Response headers
section_request_headers = Request headers
section_details = Details
detail_url = URL
detail_version = Version
detail_remote_address = Remote address
detail_time_to_headers = Time to headers
detail_dns = DNS lookup
timeline_none = This response has no recorded timeline.
body_truncated = (The body is longer than shown here.)
body_binary = (A binary body.)

# Settings ▸ Data: which store the client opened, and where it lives.
settings_data = Data
store = Store
store_shared = Shared with the Yaak desktop app
store_own = This client's own
data_dir = Data directory
