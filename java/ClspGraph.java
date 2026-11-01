/*
 * Modified copy of jadx.core.clsp.ClspGraph from JADX 1.5.6 (https://github.com/skylot/jadx).
 * Copyright 2014-2026 skylot and JADX contributors. Licensed under the Apache License 2.0.
 *
 * Changed by droidlens: the parsed Android framework class set and its super-type caches
 * are kept in static fields and shared by every JadxDecompiler in the JVM, so they are
 * parsed once per process instead of once per load().
 */
package jadx.core.clsp;

import java.io.IOException;
import java.util.ArrayList;
import java.util.Collections;
import java.util.HashMap;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;

import org.jetbrains.annotations.Nullable;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

import jadx.core.Consts;
import jadx.core.dex.info.MethodInfo;
import jadx.core.dex.instructions.args.ArgType;
import jadx.core.dex.nodes.ClassNode;
import jadx.core.dex.nodes.IMethodDetails;
import jadx.core.dex.nodes.RootNode;
import jadx.core.utils.exceptions.DecodeException;
import jadx.core.utils.exceptions.JadxRuntimeException;

/**
 * Classes hierarchy graph with methods additional info
 *
 * Patched by droidlens (from jadx 1.5.6, Apache-2.0): the framework class set and its super-type and
 * implementation caches are the same for every RootNode, so they are built once per JVM and shared. Each
 * instance then adds only its app classes. This class sits earlier on the classpath than the jadx jar and
 * replaces the original. Falls back to the original full rebuild when an app class could change the
 * framework part (the app defines a name the framework caches mention).
 */
public class ClspGraph {
	private static final Logger LOG = LoggerFactory.getLogger(ClspGraph.class);

	private final RootNode root;
	private Map<String, ClspClass> nameMap;
	private Map<String, Set<String>> superTypesCache;
	private Map<String, List<String>> implementsCache;

	private final Set<String> missingClasses = new HashSet<>();

	private static Map<String, Set<String>> baseSuperTypes;
	private static Map<String, List<String>> baseImplements;
	private static Set<String> baseMentioned;
	private static Map<String, ClspClass> baseNameMap;
	private final List<String> appNames = new ArrayList<>();
	private boolean baseLoaded;

	public ClspGraph(RootNode rootNode) {
		this.root = rootNode;
	}

	public void loadClsSetFile() throws IOException, DecodeException {
		synchronized (ClspGraph.class) {
			if (baseNameMap != null) {
				nameMap = new HashMap<>(baseNameMap);
			} else {
				ClsSet set = new ClsSet(root);
				set.loadFromClstFile();
				addClasspath(set);
				baseNameMap = new HashMap<>(nameMap);
			}
			if (baseSuperTypes == null) {
				fillSuperTypesCache();
				fillImplementsCache();
				Set<String> mentioned = new HashSet<>(superTypesCache.keySet());
				superTypesCache.values().forEach(mentioned::addAll);
				baseSuperTypes = superTypesCache;
				baseImplements = implementsCache;
				baseMentioned = mentioned;
			}
			baseLoaded = true;
		}
	}

	public void addClasspath(ClsSet set) {
		if (nameMap == null) {
			nameMap = new HashMap<>(set.getClassesCount());
			set.addToMap(nameMap);
		} else {
			throw new JadxRuntimeException("Classpath already loaded");
		}
	}

	public void addApp(List<ClassNode> classes) {
		if (nameMap == null) {
			nameMap = new HashMap<>(classes.size());
		}
		for (ClassNode cls : classes) {
			addClass(cls);
		}
	}

	public void initCache() {
		if (!baseLoaded || appNames.stream().anyMatch(baseMentioned::contains)) {
			fillSuperTypesCache();
			fillImplementsCache();
			return;
		}
		Map<String, Set<String>> superTypes = new HashMap<>(baseSuperTypes);
		for (String name : appNames) {
			ClspClass cls = nameMap.get(name);
			superTypes.put(cls.getName(), superTypesOf(cls));
		}
		superTypesCache = superTypes;
		Map<String, List<String>> impl = new HashMap<>(baseImplements);
		Set<String> touched = new HashSet<>();
		for (String name : appNames) {
			for (String st : getSuperTypes(name)) {
				if (touched.add(st)) {
					List<String> base = impl.get(st);
					impl.put(st, base == null ? new ArrayList<>() : new ArrayList<>(base));
				}
				impl.get(st).add(name);
			}
		}
		for (String st : touched) {
			Collections.sort(impl.get(st));
		}
		implementsCache = impl;
	}

	public boolean isClsKnown(String fullName) {
		return nameMap.containsKey(fullName);
	}

	public ClspClass getClsDetails(ArgType type) {
		return nameMap.get(type.getObject());
	}

	@Nullable
	public IMethodDetails getMethodDetails(MethodInfo methodInfo) {
		ClspClass cls = nameMap.get(methodInfo.getDeclClass().getRawName());
		if (cls == null) {
			return null;
		}
		ClspMethod clspMethod = getMethodFromClass(cls, methodInfo);
		if (clspMethod != null) {
			return clspMethod;
		}
		// deep search
		for (ArgType parent : cls.getParents()) {
			ClspClass clspParent = getClspClass(parent);
			if (clspParent != null) {
				ClspMethod methodFromParent = getMethodFromClass(clspParent, methodInfo);
				if (methodFromParent != null) {
					return methodFromParent;
				}
			}
		}
		// unknown method
		return new SimpleMethodDetails(methodInfo);
	}

	private ClspMethod getMethodFromClass(ClspClass cls, MethodInfo methodInfo) {
		return cls.getMethodsMap().get(methodInfo.getShortId());
	}

	private void addClass(ClassNode cls) {
		ArgType clsType = cls.getClassInfo().getType();
		String rawName = clsType.getObject();
		ClspClass clspClass = new ClspClass(clsType, -1, cls.getAccessFlags().rawValue(), ClspClassSource.APP);
		clspClass.setParents(ClsSet.makeParentsArray(cls));
		nameMap.put(rawName, clspClass);
		appNames.add(rawName);
	}

	/**
	 * @return {@code clsName} instanceof {@code implClsName}
	 */
	public boolean isImplements(String clsName, String implClsName) {
		Set<String> anc = getSuperTypes(clsName);
		return anc.contains(implClsName);
	}

	public List<String> getImplementations(String clsName) {
		List<String> list = implementsCache.get(clsName);
		return list == null ? Collections.emptyList() : list;
	}

	private void fillImplementsCache() {
		Map<String, List<String>> map = new HashMap<>(nameMap.size());
		List<String> classes = new ArrayList<>(nameMap.keySet());
		Collections.sort(classes);
		for (String cls : classes) {
			for (String st : getSuperTypes(cls)) {
				map.computeIfAbsent(st, v -> new ArrayList<>()).add(cls);
			}
		}
		implementsCache = map;
	}

	public String getCommonAncestor(String clsName, String implClsName) {
		if (clsName.equals(implClsName)) {
			return clsName;
		}
		ClspClass cls = nameMap.get(implClsName);
		if (cls == null) {
			missingClasses.add(clsName);
			return null;
		}
		if (isImplements(clsName, implClsName)) {
			return implClsName;
		}
		Set<String> anc = getSuperTypes(clsName);
		return searchCommonParent(anc, cls);
	}

	private String searchCommonParent(Set<String> anc, ClspClass cls) {
		for (ArgType p : cls.getParents()) {
			String name = p.getObject();
			if (anc.contains(name)) {
				return name;
			}
			ClspClass nCls = getClspClass(p);
			if (nCls != null) {
				String r = searchCommonParent(anc, nCls);
				if (r != null) {
					return r;
				}
			}
		}
		return null;
	}

	public Set<String> getSuperTypes(String clsName) {
		Set<String> result = superTypesCache.get(clsName);
		return result == null ? Collections.emptySet() : result;
	}

	private static final Set<String> OBJECT_SINGLE_SET = Collections.singleton(Consts.CLASS_OBJECT);

	private void fillSuperTypesCache() {
		Map<String, Set<String>> map = new HashMap<>(nameMap.size());
		for (Map.Entry<String, ClspClass> entry : nameMap.entrySet()) {
			ClspClass cls = entry.getValue();
			map.put(cls.getName(), superTypesOf(cls));
		}
		superTypesCache = map;
	}

	private Set<String> superTypesOf(ClspClass cls) {
		Set<String> tmpSet = new HashSet<>();
		addSuperTypes(cls, tmpSet);
		switch (tmpSet.size()) {
			case 0:
				return Collections.emptySet();
			case 1: {
				String supCls = tmpSet.iterator().next();
				return supCls.equals(Consts.CLASS_OBJECT) ? OBJECT_SINGLE_SET : Collections.singleton(supCls);
			}
			default:
				return tmpSet;
		}
	}

	private void addSuperTypes(ClspClass cls, Set<String> result) {
		for (ArgType parentType : cls.getParents()) {
			if (parentType == null) {
				continue;
			}
			ClspClass parentCls = getClspClass(parentType);
			if (parentCls != null) {
				boolean isNew = result.add(parentCls.getName());
				if (isNew) {
					addSuperTypes(parentCls, result);
				}
			} else {
				// parent type is unknown
				result.add(parentType.getObject());
			}
		}
	}

	@Nullable
	private ClspClass getClspClass(ArgType clsType) {
		ClspClass clspClass = nameMap.get(clsType.getObject());
		if (clspClass == null) {
			missingClasses.add(clsType.getObject());
		}
		return clspClass;
	}

	public void printMissingClasses() {
		int count = missingClasses.size();
		if (count == 0) {
			return;
		}
		LOG.warn("Found {} references to unknown classes", count);
		if (LOG.isDebugEnabled()) {
			List<String> clsNames = new ArrayList<>(missingClasses);
			Collections.sort(clsNames);
			for (String cls : clsNames) {
				LOG.debug("  {}", cls);
			}
		}
	}
}
