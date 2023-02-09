-- Copyright 2015 J.C. Moyer
--
-- Licensed under the Apache License, Version 2.0 (the "License");
-- you may not use this file except in compliance with the License.
-- You may obtain a copy of the License at
--
--     http://www.apache.org/licenses/LICENSE-2.0
--
-- Unless required by applicable law or agreed to in writing, software
-- distributed under the License is distributed on an "AS IS" BASIS,
-- WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
-- See the License for the specific language governing permissions and
-- limitations under the License.

local function escapequotes(s)
    return s:gsub('"', '\\"')
  end
  
  local html = {}
  local element = {}
  
  -- contains metadata for tags
  -- right now the only metadata stored is whether or not an element is a void
  -- element
  -- TODO: instantiable elementspecs? 
  local elementspec = { specs = {} }
  
  function elementspec.define(name, void)
    local instance = {
      name = name,
      void = void
    }
    instance.__index = instance
    elementspec.specs[name] = setmetatable(instance, element)
    return instance
  end
  
  function elementspec.get(name)
    return elementspec.specs[name]
  end
  
  element.__index = element
  
  function element.new(name)
    local instance = {
      children = {},
      attrs = {}
    }
    return setmetatable(instance, elementspec.get(name))
  end
  
  function element.fromstring(s, name)
    local e = element.new(name)
    e:appendchild(s)
    return e
  end
  
  function element.fromtable(t, name)
    local e = element.new(name)
    for k,v in pairs(t) do
      if type(k) == 'number' then
        e:appendchild(v)
      else
        e:addattr(k, v)
      end
    end
    return e
  end
  
  function element.fromvalue(x, name)
    if type(x) == 'table' then
      return element.fromtable(x, name)
    elseif type(x) == 'nil' then
      return element.fromtable({}, name)
    else
      return element.fromstring(tostring(x), name)
    end
  end
  
  function element:opentag()
    local t = {}
    for k,v in pairs(self.attrs) do
      if type(v) == 'string' then
        v = escapequotes(v)
      end
      table.insert(t, k .. '=' .. '"' .. v .. '"')
    end
    if #t > 0 then
      return '<' .. self.name .. ' ' .. table.concat(t, ' ') .. '>'
    else
      return '<' .. self.name .. '>'
    end
  end
  
  function element:closetag()
    return '</' .. self.name .. '>'
  end
  
  function element:appendchild(data)
    table.insert(self.children, data)
  end
  
  function element:addattr(attr, value)
    self.attrs[attr] = value
  end
  
  function element:render()
    local t = {}
    table.insert(t, self:opentag())  
    if not self.void then
      for i = 1, #self.children do
        local child = self.children[i]
        if element.type(child) then
          table.insert(t, child:render())
        else
          table.insert(t, tostring(child))
        end
      end
      table.insert(t, self:closetag())
    end
    return table.concat(t)
  end
  
  function element.type(t)
    -- we have to climb the __index chain because it's likely that we were given
    -- a table whose mt is an elementspec
    local mt = getmetatable(t)
    while mt do
      if mt == element then
        return 'element'
      end
      mt = getmetatable(mt)
    end
  end
  
  -- defines an element by adding an elementspec for the given name and a
  -- corresponding factory function in the html module
  -- TODO: should elementspec mt declaration be moved here?
  local function defel(name, void)
    elementspec.define(name, void)
    html[name] = function(x)
      return element.fromvalue(x, name)
    end
  end
  
  local function defels(t)
    for _,v in ipairs(t) do
      if type(v) == 'table' then
        defel(v[1] or v.name, v[2] or v.void)
      else
        defel(v, false)
      end
    end
  end
  
  function html:__call(t)
    return self.html(t)
  end
  
  function html.rendermixed(t)
    local u = {}
    for _,v in ipairs(t) do
      if element.type(v) then
        table.insert(u, v:render())
      else
        table.insert(u, tostring(v))
      end
    end
    return table.concat(u)
  end
  
  -- Elements: http://www.w3.org/TR/html5/
  -- Void elements: http://www.w3.org/TR/html5/syntax.html#elements-0
  defels {
    -- 4.1 The root element
    'html',
  
    -- 4.2 Document metadata
    'head', 'title', {'base', true}, {'link', true}, {'meta', true}, 'style',
  
    -- 4.3 Sections
    'body', 'article', 'section', 'nav', 'aside', 'h1', 'h2', 'h3', 'h4', 'h5',
    'h6', 'header', 'footer', 'address',
  
    -- 4.4 Grouping content
    'p', {'hr', true}, 'pre', 'blockquote', 'ol', 'ul', 'li', 'dl', 'dt', 'dd',
    'figure', 'figcaption', 'div', 'main',
  
    -- 4.5 Text-level semantics
    'a', 'em', 'strong', 'small', 's', 'cite', 'q', 'dfn', 'abbr', 'data',
    'time', 'code', 'var', 'samp', 'kbd', 'sub', 'sup', 'i', 'b', 'u', 'mark',
    'ruby', 'rt', 'rtc', 'rp', 'bdi', 'bdo', 'span', {'br', true}, {'wbr', true},
  
    -- 4.6 Edits
    'ins', 'del', 
  
    -- 4.7 Embedded content
    {'img', true}, 'iframe', {'embed', true}, 'object', {'param', true}, 'video',
    'audio', {'source', true}, {'track', true}, 'map', {'area', true}, 
  
    -- 4.9 Tabular data
    'table', 'caption', 'colgroup', {'col', true}, 'tbody', 'thead', 'tfoot',
    'tr', 'td', 'th', 
  
    -- 4.10 Forms
    'form', 'label', {'input', true}, 'button', 'select', 'datalist', 'optgroup',
    'option', 'textarea', {'keygen', true}, 'output', 'progress', 'meter',
    'fieldset', 'legend',
  
    -- 4.11 Scripting
    'script', 'noscript', 'template', 'canvas'
  }
  
  html.element = element
  return setmetatable(html, html)